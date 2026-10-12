// Only in a plain browser: draws the app inside a frame of the real
// window size and adds a strip of review controls under it. None of this
// ships in the Tauri window.

import { useEffect, useState } from "react";
import { App } from "./App";
import { ApprovalView } from "./approval/ApprovalView";
import { mockBackend } from "./backend";
import { resolveLang, stringsFor } from "./i18n";
import { StoreProvider } from "./lib/store";
import type { Step } from "./screens/Onboarding";

const params = new URLSearchParams(window.location.search);
const STEPS: Step[] = ["welcome", "code", "confirm", "preset", "folders", "computer", "done"];

function setParam(key: string, value: string | null) {
  const next = new URLSearchParams(window.location.search);
  if (value === null) next.delete(key);
  else next.set(key, value);
  window.location.search = next.toString();
}

export function MockStage(props: { approvalOnly?: boolean }) {
  const approvalOnly = props.approvalOnly || params.has("approval");
  const [pending, setPending] = useState(0);
  const [working, setWorking] = useState(mockBackend.working);
  const [dropped, setDropped] = useState(false);
  const s = stringsFor(resolveLang(params.get("lang"))).mock;
  const stepParam = Number(params.get("step"));
  const initialStep = params.get("screen") === "onboarding" ? STEPS[Math.min(7, Math.max(1, stepParam || 1)) - 1] : undefined;

  useEffect(() => {
    const refresh = () => void mockBackend.approvalsPending().then((list) => setPending(list.length));
    refresh();
    return mockBackend.subscribe((event) => {
      if (event.type === "approval" || event.type === "approval_closed") refresh();
    });
  }, []);

  const requests: [string, string][] = [
    ["net", "site"],
    ["cookies", "sign-in"],
    ["computer", "task"],
    ["foreground", "screen"],
    ["files", "file"],
    ["other", "unknown"],
  ];

  return (
    <div className="mock-stage">
      {approvalOnly ? (
        <div className="mock-window small">
          <StoreProvider main={false}>
            <ApprovalView />
          </StoreProvider>
        </div>
      ) : (
        <>
          <div className="mock-window">
            <StoreProvider>
              <App initialScreen={params.get("screen")} initialStep={initialStep} />
            </StoreProvider>
          </div>
          {pending > 0 ? (
            <>
              <div className="mock-caption">{s.window}</div>
              <div className="mock-window small">
                <StoreProvider main={false}>
                  <ApprovalView />
                </StoreProvider>
              </div>
            </>
          ) : null}
        </>
      )}

      <div className="mock-note">{s.label}</div>
      <div className="mock-bar">
        {requests.map(([kind, label]) => (
          <button key={kind} type="button" className="btn small" onClick={() => mockBackend.pushApproval(kind)}>
            {s.request}: {label}
          </button>
        ))}
      </div>
      <div className="mock-bar">
        <button type="button" className="btn small" onClick={() => mockBackend.simulateActivity()}>
          {s.activity}
        </button>
        <button
          type="button"
          className="btn small"
          onClick={() => {
            if (working) mockBackend.stopWork();
            else mockBackend.startWork();
            setWorking(!working);
          }}
        >
          {working ? s.stopWork : s.work}
        </button>
        <button
          type="button"
          className="btn small"
          onClick={() => {
            mockBackend.simulateLink(dropped ? "linked" : "reconnecting");
            setDropped(!dropped);
          }}
        >
          {dropped ? s.restore : s.drop}
        </button>
        <button type="button" className="btn small" onClick={() => mockBackend.simulateDeepLink()}>
          {s.link}
        </button>
        <button type="button" className="btn small" onClick={() => mockBackend.simulateUpdate()}>
          {s.update}
        </button>
        <button type="button" className="btn small" onClick={() => mockBackend.simulateHelper("install")}>
          {s.helperInstall}
        </button>
        <button type="button" className="btn small" onClick={() => mockBackend.simulateHelper("fail")}>
          {s.helperFail}
        </button>
        <button type="button" className="btn small" onClick={() => setParam("lang", params.get("lang") === "ru" ? "en" : "ru")}>
          EN / RU
        </button>
        <button type="button" className="btn small" onClick={() => setParam("theme", params.get("theme") === "dark" ? "light" : "dark")}>
          Light / Dark
        </button>
      </div>
    </div>
  );
}
