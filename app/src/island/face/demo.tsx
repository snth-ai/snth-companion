import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { FaceDemo } from "./FaceDemo";
import "../../styles/app.css";

const theme = new URLSearchParams(window.location.search).get("theme");
if (theme === "dark" || theme === "light") document.documentElement.dataset.theme = theme;

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <FaceDemo />
  </StrictMode>,
);
