import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { isMock } from "./backend";
import { StoreProvider } from "./lib/store";
import { MockStage } from "./MockStage";
import "./styles/app.css";

// Review aid: ?theme=dark|light forces a theme in the browser mock.
if (isMock) {
  const theme = new URLSearchParams(window.location.search).get("theme");
  if (theme === "dark" || theme === "light") document.documentElement.dataset.theme = theme;
}

// The webview's own right-click menu (Reload, Inspect) has no place in the
// app; text fields and selectable text keep theirs.
if (!isMock) {
  window.addEventListener("contextmenu", (e) => {
    if (!(e.target as HTMLElement).closest("input, .selectable")) e.preventDefault();
  });
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {isMock ? (
      <MockStage />
    ) : (
      <StoreProvider>
        <App />
      </StoreProvider>
    )}
  </StrictMode>,
);
