import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { isMock } from "./backend";
import { IslandApp } from "./island/IslandApp";
import { IslandStage } from "./island/IslandStage";
import "./island/island.css";

// The review stage borrows the app's buttons; the real island has no use
// for the app's stylesheet (it would paint the transparent window).
if (isMock) void import("./styles/app.css");

// No context menu, no text selection outside the summary: the island is
// not a page.
if (!isMock) {
  window.addEventListener("contextmenu", (e) => {
    if (!(e.target as HTMLElement).closest("input, .selectable")) e.preventDefault();
  });
}

createRoot(document.getElementById("root")!).render(<StrictMode>{isMock ? <IslandStage /> : <IslandApp />}</StrictMode>);
