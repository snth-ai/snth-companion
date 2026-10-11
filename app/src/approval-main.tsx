import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { ApprovalView } from "./approval/ApprovalView";
import { isMock, mockBackend } from "./backend";
import { StoreProvider } from "./lib/store";
import { MockStage } from "./MockStage";
import "./styles/app.css";

if (isMock) {
  const params = new URLSearchParams(window.location.search);
  const theme = params.get("theme");
  if (theme === "dark" || theme === "light") document.documentElement.dataset.theme = theme;
  // Opened directly in a browser: show one sample request.
  if (!params.has("approval")) mockBackend.pushApproval("cookies", false);
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    {isMock ? (
      <MockStage approvalOnly />
    ) : (
      <StoreProvider main={false}>
        <ApprovalView />
      </StoreProvider>
    )}
  </StrictMode>,
);
