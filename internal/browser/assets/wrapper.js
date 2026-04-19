// Invocation bundle. dom_tree.js (from alibaba/page-agent, which in
// turn is forked from browser-use) is concatenated before this file
// by snapshot.go. The bundled blob is passed to Runtime.evaluate
// in one shot.
//
// dom_tree.js was sed-converted from `export default (args = {...})`
// to `(args = {...})`, leaving it as a naked arrow-function
// expression. We capture its return value into __snth_tree and stash
// it on window for action resolution.
//
// Defaults must include every top-level key the extractor
// destructures — JS default parameter only fires when the arg is
// `undefined`, so passing a partial object means missing keys come
// through as `undefined` and blow up later (isInteractiveElement
// crashes on `interactiveBlacklist.includes(el)`).

;(() => {
  try {
    const __snth_fn = __SNTH_DOM_TREE_FN__
    const tree = __snth_fn({
      doHighlightElements: false,
      focusHighlightIndex: -1,
      viewportExpansion: 0,
      debugMode: false,
      interactiveBlacklist: [],
      interactiveWhitelist: [],
      highlightOpacity: 0.1,
      highlightLabelOpacity: 0.5,
    })
    window.__snth_tree = tree
    return JSON.stringify({
      rootId: tree.rootId,
      map: tree.map,
      title: document.title,
      url: location.href,
    })
  } catch (e) {
    return JSON.stringify({ error: String(e && e.stack || e) })
  }
})();
