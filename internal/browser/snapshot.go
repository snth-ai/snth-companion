package browser

import (
	"context"
	_ "embed"
	"encoding/json"
	"fmt"
	"sort"
	"strings"
)

// snapshot.go — DOM → compact LLM-friendly representation.
//
// We use alibaba/page-agent's battle-tested DOM extractor (itself
// forked from browser-use). The JS walks the DOM, detects
// interactivity via ~40 heuristics (visibility + event handlers +
// cursor style + ARIA + role + etc.), assigns stable highlightIndex
// numbers, and returns a flat hash map. We receive that JSON, then
// format it server-side (Go) so we can tune the output for our LLM
// budget without re-running JS.
//
// Output has two halves:
//   - `text`: the indented "[12]<button ...>Submit</button>" lines
//     the LLM sees. Matches page-agent's format closely.
//   - `selectors`: map[highlightIndex]metadata for click/type to
//     resolve clicks via Runtime.evaluate(window.__snth_tree.map[id]).

//go:embed assets/dom_tree.js
var domTreeJS string

//go:embed assets/wrapper.js
var wrapperJS string

// SnapshotResult is what we hand back to the browser tool. The
// companion sends it to the synth verbatim.
type SnapshotResult struct {
	Title     string                    `json:"title"`
	URL       string                    `json:"url"`
	Text      string                    `json:"text"`      // indented [idx]<tag...> lines
	Selectors map[int]SelectorEntry     `json:"selectors"` // highlightIndex → metadata
}

// SelectorEntry is the per-ref metadata used by actions.go to
// resolve clicks, typing, etc. We keep it small so snapshot size
// stays bounded — raw DOM isn't included, just what we need to
// dispatch events.
type SelectorEntry struct {
	Tag        string            `json:"tag"`
	Attributes map[string]string `json:"attributes,omitempty"`
	NodeID     string            `json:"node_id"`
	Text       string            `json:"text,omitempty"`
}

// buildSnapshotBundle splices the page-agent extractor into the
// wrapper IIFE. Same bundle is consumed by the CDP backend (this file's
// Snapshot) and the Playwright backend (playwright_worker.go's
// PWSnapshot). Both call paths get an identical FlatDomTree on the
// wire — no DOM logic is duplicated.
func buildSnapshotBundle() string {
	return strings.Replace(wrapperJS, "__SNTH_DOM_TREE_FN__", "("+domTreeJS+")", 1)
}

// Snapshot runs the embedded JS, parses the FlatDomTree map, and
// formats the text. Stashes window.__snth_tree on the page so
// click/type can resolve refs in a second Runtime.evaluate.
func Snapshot(ctx context.Context, c Conn) (*SnapshotResult, error) {
	if err := enableRuntime(ctx, c); err != nil {
		return nil, err
	}

	// The dom_tree.js was sed-converted from `export default (args =...)`
	// to a naked arrow-function expression. We splice it in at the
	// placeholder in wrapper.js and eval the combined source.
	bundle := buildSnapshotBundle()

	var resp evalResult
	err := c.Send(ctx, "Runtime.evaluate", map[string]any{
		"expression":    bundle,
		"returnByValue": true,
		"awaitPromise":  false,
	}, &resp)
	if err != nil {
		return nil, fmt.Errorf("snapshot eval: %w", err)
	}
	if resp.ExceptionDetails != nil {
		return nil, fmt.Errorf("snapshot eval exception: %s", resp.ExceptionDetails.Text)
	}
	raw, ok := resp.Result.Value.(string)
	if !ok {
		blob, err := json.Marshal(resp.Result.Value)
		if err != nil {
			return nil, fmt.Errorf("snapshot result: unexpected type %T", resp.Result.Value)
		}
		raw = string(blob)
	}

	var tree flatTree
	if err := json.Unmarshal([]byte(raw), &tree); err != nil {
		return nil, fmt.Errorf("snapshot decode: %w (raw head: %s)", err, head(raw, 200))
	}
	if tree.Err != "" {
		return nil, fmt.Errorf("snapshot JS error: %s", tree.Err)
	}

	text, selectors := formatTree(&tree)
	return &SnapshotResult{
		Title:     tree.Title,
		URL:       tree.URL,
		Text:      text,
		Selectors: selectors,
	}, nil
}

// --- wire types ------------------------------------------------------------
//
// These mirror what dom_tree.js (via wrapper.js) puts on the wire. Every
// field is optional; absent fields are zero-value. We keep the struct
// permissive because upstream page-agent can add fields without breaking us.

type flatTree struct {
	RootID string                 `json:"rootId"`
	Map    map[string]flatNode    `json:"map"`
	Title  string                 `json:"title"`
	URL    string                 `json:"url"`
	Err    string                 `json:"error,omitempty"`
}

type flatNode struct {
	// Common
	Type string `json:"type"` // "TEXT_NODE" or element (blank = element for dom_tree.js)

	// Element
	TagName        string            `json:"tagName,omitempty"`
	Attributes     map[string]string `json:"attributes,omitempty"`
	Children       []string          `json:"children,omitempty"`
	IsVisible      bool              `json:"isVisible,omitempty"`
	IsInteractive  bool              `json:"isInteractive,omitempty"`
	IsTopElement   bool              `json:"isTopElement,omitempty"`
	IsNew          bool              `json:"isNew,omitempty"`
	HighlightIndex *int              `json:"highlightIndex,omitempty"`
	Extra          map[string]any    `json:"extra,omitempty"`

	// Text
	Text string `json:"text,omitempty"`
}

// --- formatter -------------------------------------------------------------

// formatTree walks the flat map and builds:
//   - indented [idx]<tag ...>text</tag> lines for every element with a
//     highlightIndex, plus the text nodes that "anchor" them.
//   - the selector map Go-side so actions can resolve ref → node_id.
//
// Lightweight port of page-agent's flatTreeToString — same shape,
// simpler attr-cleanup rules. If we hit output-quality problems we can
// bring back the duplicate-value elimination + semantic-tag emission.
func formatTree(tree *flatTree) (string, map[int]SelectorEntry) {
	if tree.RootID == "" || tree.Map == nil {
		return "", nil
	}
	selectors := map[int]SelectorEntry{}
	var lines []string

	// Attributes we care about when printing. Intentionally short —
	// every extra attr eats tokens.
	allowAttrs := []string{
		"type", "role", "name", "aria-label", "placeholder",
		"value", "title", "alt", "href", "checked",
		"aria-expanded", "aria-checked", "aria-haspopup",
	}
	allow := map[string]bool{}
	for _, a := range allowAttrs {
		allow[a] = true
	}

	var walk func(id string, depth int)
	walk = func(id string, depth int) {
		node, ok := tree.Map[id]
		if !ok {
			return
		}
		nextDepth := depth

		if node.Type == "" || node.Type != "TEXT_NODE" {
			// Element.
			if node.HighlightIndex != nil {
				idx := *node.HighlightIndex
				prefix := "["
				if node.IsNew {
					prefix = "*["
				}
				textInside := collectText(tree, id)
				attrsStr := ""
				for _, a := range allowAttrs {
					if v, ok := node.Attributes[a]; ok && v != "" {
						if attrsStr != "" {
							attrsStr += " "
						}
						attrsStr += fmt.Sprintf("%s=%s", a, capAttr(v, 24))
					}
				}
				line := strings.Repeat("\t", depth) + prefix + fmt.Sprintf("%d]", idx) + "<" + node.TagName
				if attrsStr != "" {
					line += " " + attrsStr
				}
				if textInside != "" {
					line += ">" + textInside + "</" + node.TagName + ">"
				} else {
					line += " />"
				}
				lines = append(lines, line)

				selectors[idx] = SelectorEntry{
					Tag:        node.TagName,
					Attributes: filterAttrs(node.Attributes, allow),
					NodeID:     id,
					Text:       textInside,
				}
				nextDepth++
			}
			for _, cid := range node.Children {
				walk(cid, nextDepth)
			}
		} else {
			// Text node: only surface it if there's no highlighted
			// ancestor (i.e. not already captured inside a [idx]).
			if !hasHighlightedAncestor(tree, id) {
				t := strings.TrimSpace(node.Text)
				if t != "" {
					lines = append(lines, strings.Repeat("\t", depth)+capAttr(t, 200))
				}
			}
		}
	}

	walk(tree.RootID, 0)
	return strings.Join(lines, "\n"), selectors
}

func collectText(tree *flatTree, id string) string {
	var parts []string
	var walk func(nid string, depth int)
	walk = func(nid string, depth int) {
		n, ok := tree.Map[nid]
		if !ok {
			return
		}
		if n.Type == "TEXT_NODE" && strings.TrimSpace(n.Text) != "" {
			parts = append(parts, strings.TrimSpace(n.Text))
			return
		}
		if n.Type == "" || n.Type != "TEXT_NODE" {
			// Descend into children UNLESS the child is itself
			// a highlighted interactive element (we don't want to
			// slurp neighbour labels into our name).
			for _, cid := range n.Children {
				c, ok := tree.Map[cid]
				if !ok {
					continue
				}
				if c.Type != "TEXT_NODE" && c.HighlightIndex != nil && cid != id {
					continue
				}
				walk(cid, depth+1)
			}
		}
	}
	walk(id, 0)
	joined := strings.Join(parts, " ")
	joined = strings.Join(strings.Fields(joined), " ")
	return capAttr(joined, 120)
}

func hasHighlightedAncestor(tree *flatTree, id string) bool {
	// Cheap parent-walk by scanning children lists. O(N²) worst case
	// but N is small enough (< few thousand nodes) that it doesn't
	// hurt in practice. A proper port would build a parent map once.
	for parentID, node := range tree.Map {
		for _, cid := range node.Children {
			if cid == id {
				if node.HighlightIndex != nil {
					return true
				}
				return hasHighlightedAncestor(tree, parentID)
			}
		}
	}
	return false
}

func filterAttrs(in map[string]string, allow map[string]bool) map[string]string {
	out := map[string]string{}
	keys := make([]string, 0, len(in))
	for k := range in {
		if allow[k] {
			keys = append(keys, k)
		}
	}
	sort.Strings(keys)
	for _, k := range keys {
		out[k] = in[k]
	}
	return out
}

func capAttr(s string, n int) string {
	s = strings.ReplaceAll(s, "\n", " ")
	if len(s) <= n {
		return s
	}
	return s[:n-3] + "..."
}

// --- evaluation plumbing ---------------------------------------------------

type evalResult struct {
	Result struct {
		Type        string      `json:"type"`
		Subtype     string      `json:"subtype,omitempty"`
		Value       interface{} `json:"value,omitempty"`
		Description string      `json:"description,omitempty"`
		ObjectID    string      `json:"objectId,omitempty"`
	} `json:"result"`
	ExceptionDetails *struct {
		Text      string `json:"text"`
		Exception *struct {
			Description string `json:"description"`
		} `json:"exception,omitempty"`
	} `json:"exceptionDetails,omitempty"`
}

// enableRuntime enables the Runtime domain on the attached conn.
// Idempotent per CDP docs — calling it repeatedly is cheap and
// mandatory before Runtime.evaluate works on a fresh session.
// Accepts Conn so it works with either the direct CDP or the
// extension-relay transport.
func enableRuntime(ctx context.Context, c Conn) error {
	if err := c.Send(ctx, "Runtime.enable", nil, nil); err != nil {
		return fmt.Errorf("Runtime.enable: %w", err)
	}
	return nil
}

func head(s string, n int) string {
	if len(s) <= n {
		return s
	}
	return s[:n-1] + "…"
}

func quoteJSString(s string) string {
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range s {
		switch r {
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		case '\n':
			b.WriteString(`\n`)
		case '\r':
			b.WriteString(`\r`)
		case '\t':
			b.WriteString(`\t`)
		default:
			if r < 0x20 {
				fmt.Fprintf(&b, `\u%04x`, r)
			} else {
				b.WriteRune(r)
			}
		}
	}
	b.WriteByte('"')
	return b.String()
}
