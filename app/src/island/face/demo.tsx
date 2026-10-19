import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { FaceDemo } from "./FaceDemo";
import "../../styles/app.css";

// ?perf=1: count animation frames the faces ask for (energy check).
if (new URLSearchParams(window.location.search).get("perf") === "1") {
  const raw = window.requestAnimationFrame.bind(window);
  const stamps: number[] = [];
  window.requestAnimationFrame = (cb) => {
    stamps.push(performance.now());
    return raw(cb);
  };
  setInterval(() => {
    const now = performance.now();
    const last = (s: number) => stamps.filter((x) => x > now - s * 1000).length;
    document.title = `raf/s 1s=${last(1)} 10s=${(last(10) / 10).toFixed(1)}`;
  }, 1000);
}

const theme = new URLSearchParams(window.location.search).get("theme");
if (theme === "dark" || theme === "light") document.documentElement.dataset.theme = theme;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <FaceDemo />
  </StrictMode>,
);
