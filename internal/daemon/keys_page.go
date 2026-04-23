package daemon

// keys_page.go — "API Keys" tab in the companion UI. Lets the paired
// user upload their own API key for any supported provider (not Codex
// OAuth — that has its own /login/codex page). The form asks for
// provider + api_key + model; the hub validates the key against the
// provider's /models (or count_tokens for Anthropic) before the vault
// ever sees it, so a bad paste is rejected inline.
//
// State shown: current primary {provider, model, key_label}. If the
// key label ends with "(<synth_id>)" it's the user's own upload — the
// "now using your key" badge lights up. Otherwise it's a shared or
// operator-assigned key (the default Mia ships with).

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/snth-ai/snth-companion/internal/config"
)

type providerCatalogEntry struct {
	Provider     string `json:"provider"`
	Display      string `json:"display"`
	ExampleModel string `json:"example_model"`
	DocsURL      string `json:"docs_url"`
	Hint         string `json:"hint"`
}

type llmConfigResp struct {
	SynthID string `json:"synth_id"`
	Primary *struct {
		Provider       string `json:"provider"`
		Model          string `json:"model"`
		KeyLabel       string `json:"key_label"`
		IsUserUploaded bool   `json:"is_user_uploaded"`
	} `json:"primary,omitempty"`
}

func (s *UIServer) handleKeysPage(w http.ResponseWriter, r *http.Request) {
	cfg := config.Get()
	paired := cfg != nil && cfg.CompanionToken != ""
	if !paired {
		s.layout(w, "/keys", "API Keys",
			`<div class="card"><h2>Not paired</h2><p style="color:#94a3b8;font-size:13px">Pair the companion first — API-key upload is scoped to the paired synth.</p></div>`)
		return
	}
	hubURL := cfg.HubURL
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}

	catalog, catErr := fetchProviderCatalog(hubURL, cfg.CompanionToken)
	current, curErr := fetchLLMConfig(hubURL, cfg.CompanionToken)

	// Current state card.
	stateCard := ""
	if curErr != nil {
		stateCard = fmt.Sprintf(`<div class="card" style="border-color:#ef4444"><h2>Can't read current LLM config</h2><p class="mono" style="color:#fca5a5;font-size:12px">%s</p></div>`, htmlEscape(curErr.Error()))
	} else if current != nil && current.Primary != nil {
		ribbon := ""
		if current.Primary.IsUserUploaded {
			ribbon = `<span style="background:#22c55e;color:#0f172a;padding:2px 8px;border-radius:10px;font-size:11px;margin-left:8px">your key</span>`
		} else {
			ribbon = `<span style="background:#475569;color:#f8fafc;padding:2px 8px;border-radius:10px;font-size:11px;margin-left:8px">shared / operator</span>`
		}
		stateCard = fmt.Sprintf(`
<div class="card">
  <h2>Now using%s</h2>
  <div class="row"><span class="label">Provider</span><span class="mono">%s</span></div>
  <div class="row"><span class="label">Model</span><span class="mono">%s</span></div>
  <div class="row"><span class="label">Key label</span><span class="mono">%s</span></div>
</div>`, ribbon, htmlEscape(current.Primary.Provider), htmlEscape(current.Primary.Model), htmlEscape(current.Primary.KeyLabel))
	} else {
		stateCard = `<div class="card"><h2>No primary assignment</h2><p style="color:#94a3b8;font-size:13px">Your synth currently has no primary LLM assigned — upload a key below.</p></div>`
	}

	// Upload form.
	formCard := ""
	if catErr != nil {
		formCard = fmt.Sprintf(`<div class="card" style="border-color:#ef4444"><h2>Can't load provider catalog</h2><p class="mono" style="color:#fca5a5;font-size:12px">%s</p></div>`, htmlEscape(catErr.Error()))
	} else {
		opts := ""
		hints := ""
		for _, p := range catalog {
			opts += fmt.Sprintf(`<option value="%s" data-example="%s" data-hint="%s" data-docs="%s">%s</option>`,
				htmlEscape(p.Provider), htmlEscape(p.ExampleModel), htmlEscape(p.Hint), htmlEscape(p.DocsURL), htmlEscape(p.Display))
			hints += fmt.Sprintf(`<div data-for="%s" style="display:none"><p style="color:#94a3b8;font-size:12px;margin:0 0 6px 0">%s</p><p style="font-size:12px;margin:0"><a href="%s" target="_blank" rel="noopener">Docs → %s</a></p></div>`,
				htmlEscape(p.Provider), htmlEscape(p.Hint), htmlEscape(p.DocsURL), htmlEscape(p.DocsURL))
		}
		formCard = fmt.Sprintf(`
<div class="card">
  <h2>Upload your API key</h2>
  <p style="color:#94a3b8;font-size:13px">
    Your key is validated against the provider before it's stored. The synth switches to your primary immediately (env push + container recreate). Shared utility keys (Gemini vision, etc.) keep working underneath — only the primary provider slot gets your key.
  </p>
  <form method="POST" action="/keys/save" id="keyform">
    <label>Provider</label>
    <select name="provider" id="kp-provider" onchange="kpUpdateExample()">
      %s
    </select>
    <div id="kp-hint" style="margin-top:8px">%s</div>

    <label>Model id</label>
    <input name="model" id="kp-model" placeholder="" required />
    <p style="color:#64748b;font-size:11px;margin-top:2px">Free-form — whatever id the provider's docs show. OpenRouter uses <code>publisher/model</code>.</p>

    <label>API key</label>
    <input name="api_key" type="password" placeholder="sk-..." required />

    <div style="margin-top:16px"><button type="submit">Validate &amp; Apply</button></div>
  </form>
</div>
<script>
function kpUpdateExample() {
    var sel = document.getElementById('kp-provider');
    var opt = sel.options[sel.selectedIndex];
    var modelInput = document.getElementById('kp-model');
    if (modelInput && !modelInput.value) {
        modelInput.placeholder = opt.getAttribute('data-example') || '';
    }
    var prov = sel.value;
    var hintHost = document.getElementById('kp-hint');
    if (hintHost) {
        hintHost.querySelectorAll('[data-for]').forEach(function(el){
            el.style.display = (el.getAttribute('data-for') === prov) ? 'block' : 'none';
        });
    }
}
document.addEventListener('DOMContentLoaded', kpUpdateExample);
kpUpdateExample();
</script>`, opts, hints)
	}

	s.layout(w, "/keys", "API Keys", stateCard+formCard)
}

func (s *UIServer) handleKeysSave(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost {
		http.Error(w, "POST only", 405)
		return
	}
	if err := r.ParseForm(); err != nil {
		http.Error(w, "parse form: "+err.Error(), 400)
		return
	}
	cfg := config.Get()
	if cfg == nil || cfg.CompanionToken == "" {
		http.Error(w, "not paired", 400)
		return
	}
	hubURL := cfg.HubURL
	if hubURL == "" {
		hubURL = "https://hub.snth.ai"
	}
	payload, _ := json.Marshal(map[string]string{
		"provider": strings.TrimSpace(r.FormValue("provider")),
		"api_key":  strings.TrimSpace(r.FormValue("api_key")),
		"model":    strings.TrimSpace(r.FormValue("model")),
	})
	req, err := http.NewRequest("POST", strings.TrimRight(hubURL, "/")+"/api/my/provider-key", bytes.NewReader(payload))
	if err != nil {
		renderKeysError(s, w, err.Error())
		return
	}
	req.Header.Set("Authorization", "Bearer "+cfg.CompanionToken)
	req.Header.Set("Content-Type", "application/json")
	client := &http.Client{Timeout: 30 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		renderKeysError(s, w, "hub unreachable: "+err.Error())
		return
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		renderKeysError(s, w, fmt.Sprintf("hub %d: %s", resp.StatusCode, strings.TrimSpace(string(body))))
		return
	}
	http.Redirect(w, r, "/keys", http.StatusSeeOther)
}

func renderKeysError(s *UIServer, w http.ResponseWriter, msg string) {
	s.layout(w, "/keys", "API Keys",
		fmt.Sprintf(`<div class="card" style="border-color:#ef4444"><h2>✗ Upload failed</h2><p class="mono" style="color:#fca5a5;font-size:12px">%s</p><p style="margin-top:12px"><a href="/keys">← Try again</a></p></div>`, htmlEscape(msg)))
}

func fetchProviderCatalog(hubURL, token string) ([]providerCatalogEntry, error) {
	req, err := http.NewRequest("GET", strings.TrimRight(hubURL, "/")+"/api/my/provider-catalog", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	client := &http.Client{Timeout: 15 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		return nil, fmt.Errorf("hub %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}
	var out struct {
		Providers []providerCatalogEntry `json:"providers"`
	}
	if err := json.Unmarshal(body, &out); err != nil {
		return nil, err
	}
	return out.Providers, nil
}

func fetchLLMConfig(hubURL, token string) (*llmConfigResp, error) {
	req, err := http.NewRequest("GET", strings.TrimRight(hubURL, "/")+"/api/my/llm-config", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Authorization", "Bearer "+token)
	client := &http.Client{Timeout: 15 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()
	body, _ := io.ReadAll(resp.Body)
	if resp.StatusCode/100 != 2 {
		return nil, fmt.Errorf("hub %d: %s", resp.StatusCode, strings.TrimSpace(string(body)))
	}
	var out llmConfigResp
	if err := json.Unmarshal(body, &out); err != nil {
		return nil, err
	}
	return &out, nil
}
