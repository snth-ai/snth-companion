// Only in a plain browser (island.html): the island window on a pretend
// desktop with a menu bar and, on a Mac with one, the notch. The pointer
// over the desktop is the gaze; the strip under it plays the island's
// moments. None of this ships in the app.

import { useEffect, useState } from "react";
import { mockBackend } from "../backend";
import { IslandApp } from "./IslandApp";

const params = new URLSearchParams(window.location.search);

const SCENARIOS = [
  "idle",
  "hover",
  "listening",
  "screen",
  "sending",
  "accepted",
  "thinking",
  "working",
  "driving",
  "done",
  "error",
  "approval",
  "text",
  "offline",
  "outdated",
  "micdenied",
  "asking",
  "paused",
  "two",
  "shot",
  "live",
  "livesent",
  "offer",
  "offerdl",
  "offerready",
  "offerfail",
  "wakeidle",
  "wakepaused",
  "woke",
  "wokename",
];

function setParam(key: string, value: string | null) {
  const next = new URLSearchParams(window.location.search);
  if (value === null) next.delete(key);
  else next.set(key, value);
  window.location.search = next.toString();
}

export function IslandStage() {
  const win = mockBackend.islandWin;
  const p = win.view.placement!;
  const [pointer, setPointer] = useState<{ x: number; y: number } | null>(null);
  // Full screen and its menu bar live in the mock window; follow them.
  const [, setTick] = useState(0);
  useEffect(() => win.subscribe(() => setTick((t) => t + 1)), [win]);
  const barAway = win.fullScreen && !win.barShown;
  const desktopW = 1512;
  const frameLeft = desktopW / 2 - p.window.w / 2;
  const mac = win.view.os === "macos";

  return (
    <div className="isl-stage">
      <div
        className={`isl-desktop${mac ? "" : " win"}${win.fullScreen ? " full" : ""}`}
        style={{ width: desktopW }}
        onMouseMove={(e) => {
          const box = e.currentTarget.getBoundingClientRect();
          const x = e.clientX - box.left;
          const y = e.clientY - box.top;
          setPointer({ x, y });
          win.pointer(x - frameLeft, y);
          win.setHover(Boolean((e.target as HTMLElement).closest(".isl-shape")));
        }}
        onMouseLeave={() => win.setHover(false)}
      >
        {mac ? (
          <div className={`isl-menubar${barAway ? " away" : ""}`} style={{ height: p.notch.h }}>
            <span>Finder</span>
            <span>File</span>
            <span>Edit</span>
            <span>View</span>
            <span className="isl-menubar-right">Mon 21:40</span>
          </div>
        ) : null}
        {p.real_notch ? <div className="isl-hw-notch" style={{ width: p.notch.w, height: p.notch.h, left: desktopW / 2 - p.notch.w / 2 }} /> : null}
        <div className="isl-frame" style={{ left: frameLeft, width: p.window.w, height: p.window.h }}>
          <IslandApp />
        </div>
        {pointer ? null : (
          <div className="isl-stage-hint">
            {win.fullScreen
              ? "Full screen: the island is away. Move the pointer to the very top edge: the menu bar and the island come in."
              : "Move the pointer over the desktop: the eyes follow it. Hover the island to open it."}
          </div>
        )}
      </div>

      <div className="mock-note">Mock island: not the real app. The frame is the island window (transparent in the app).</div>
      <div className="mock-bar">
        {SCENARIOS.map((sc) => (
          <button key={sc} type="button" className="btn small" aria-pressed={(params.get("island") ?? "idle") === sc} onClick={() => setParam("island", sc)}>
            {sc}
          </button>
        ))}
      </div>
      <div className="mock-bar">
        <button type="button" className="btn small" onClick={() => win.playHold("voice")}>
          hold to talk (2.6 s)
        </button>
        <button type="button" className="btn small" onClick={() => win.playHold("screen_voice")}>
          hold with screenshot
        </button>
        <button type="button" className="btn small" onClick={() => win.playLiveHold()}>
          hold, words on device
        </button>
        <button type="button" className="btn small" onClick={() => win.playOffer()}>
          speech offer
        </button>
        <button type="button" className="btn small" onClick={() => win.playWake()}>
          say "Mia, ..."
        </button>
        <button type="button" className="btn small" onClick={() => win.playWake(true)}>
          say "Mia" alone
        </button>
        <button type="button" className="btn small" onClick={() => win.toggleWakePause()}>
          wake: call / no call
        </button>
        <button type="button" className="btn small" onClick={() => win.playText()}>
          type a message
        </button>
        <button type="button" className="btn small" onClick={() => win.playAnswer()}>
          answer
        </button>
        <button type="button" className="btn small" onClick={() => win.playAnswer(true)}>
          answer fails
        </button>
        <button type="button" className="btn small" onClick={() => win.playApproval()}>
          request
        </button>
        <button type="button" className="btn small" onClick={() => win.toggleDriving()}>
          drives / stops
        </button>
        <button type="button" className="btn small" onClick={() => win.toggleOffline()}>
          offline / online
        </button>
        <button type="button" className="btn small" aria-pressed={win.fullScreen} onClick={() => win.setFullScreen(!win.fullScreen)}>
          full screen
        </button>
      </div>
      <div className="mock-bar">
        <button type="button" className="btn small" onClick={() => setParam("notch", params.get("notch") === "0" ? null : "0")}>
          notch / no notch
        </button>
        <button type="button" className="btn small" onClick={() => setParam("os", params.get("os") === "windows" ? null : "windows")}>
          macOS / Windows
        </button>
        <button type="button" className="btn small" onClick={() => setParam("lang", params.get("lang") === "ru" ? "en" : "ru")}>
          EN / RU
        </button>
      </div>
    </div>
  );
}
