import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { FaceLab } from "./FaceLab";
import "./lab.css";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <FaceLab />
  </StrictMode>,
);
