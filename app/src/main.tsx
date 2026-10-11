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
