// The island window: a black shape at the top of the screen that merges
// with the notch (or is drawn as if there were one), the synth's face in
// its left ear and what it is doing in its right one. It opens up when
// the pointer is over it, while a key is held, while a request waits for
// the owner, for the line of text, and for a few seconds after an answer
// (the summary).
//
// The shell places the window and lets clicks through everywhere but the
// shape; this page says where the shape is and where the face is
// (`islandLayout`) every time they change.

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { quickAnswer } from "../approval/ApprovalView";
import { backend } from "../backend";
import type { IslandView } from "../backend/types";
import { stringsFor } from "../i18n";
import { shortcutText } from "../lib/format";
import { Face, faceFor, faceWidth, type FaceState } from "./faceApi";

/** How long the summary stays after the answer (TZ: 4-6 s). */
const SUMMARY_FOR = 6000;
const NOTICE_FOR = 6000;
/** The open island. */
const OPEN_W = 400;
/** The face in the open island: big enough for the whole head. */
const FACE_OPEN = 84;
/** How long the face takes to grow or shrink (the shape's own spring is
 * about as long). */
const FACE_MS = 420;

/** What the face shows, from what the island knows. */
export function faceStateOf(v: IslandView, now: number): FaceState {
  if (v.approval) return "approval";
  if (v.hold) return "listening";
  if (!v.synth || !v.synth.online || v.paused) return "offline";
  if (v.sending) return "thinking";
  const m = v.message;
  if (m) {
    const age = now - m.at_ms;
    if (m.stage === "accepted") return age < 800 ? "accepted" : "thinking";
    if (m.stage === "working") return v.driving ? "driving" : "working";
    if (m.stage === "done" && age < 1500) return "done";
    if (m.stage === "error" && age < 4000) return "error";
  }
  if (v.notice && now - v.notice.at_ms < 4000) return "error";
  if (v.driving) return "driving";
  return "idle";
}

function useClock(active: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(id);
  }, [active]);
  return active ? now : Date.now();
}

/** The right ear. Listening, a request, an error and driving have their
 * own symbol beside the face's right eye (the mic meter, "?", "!", the
 * pointer); the ear does not say them twice. */
function Glyph(props: { state: FaceState; sending: boolean }) {
  switch (props.state) {
    case "listening":
    case "approval":
    case "error":
    case "driving":
      return null;
    case "accepted":
    case "done":
      return (
        <svg className="isl-glyph ok" viewBox="0 0 16 16" aria-hidden="true">
          <path d="M3.5 8.5l3 3 6-7" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      );
    case "thinking":
      return (
        <span className="isl-dots" aria-hidden="true">
          <i />
          <i />
          <i />
        </span>
      );
    case "working":
      return <span className="isl-ring" aria-hidden="true" />;
    case "offline":
      return <span className="isl-dot dim" aria-hidden="true" />;
    default:
      return props.sending ? <span className="isl-ring" aria-hidden="true" /> : null;
  }
}

/** 0 (closed) .. 1 (open), eased, on animation frames while it moves. */
function useOpening(open: boolean): number {
  const [t, setT] = useState(open ? 1 : 0);
  const from = useRef(t);
  useEffect(() => {
    const target = open ? 1 : 0;
    const reduce = typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (reduce) {
      from.current = target;
      setT(target);
      return;
    }
    const start = performance.now();
    const begin = from.current;
    let frame = 0;
    const step = (now: number) => {
      const k = Math.min(1, (now - start) / FACE_MS);
      const eased = 1 - Math.pow(1 - k, 3);
      const v = begin + (target - begin) * eased;
      from.current = v;
      setT(v);
      if (k < 1) frame = requestAnimationFrame(step);
    };
    frame = requestAnimationFrame(step);
    return () => cancelAnimationFrame(frame);
  }, [open]);
  return t;
}

function mmss(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export function IslandApp() {
  const [view, setView] = useState<IslandView | null>(null);
  const [gaze, setGaze] = useState<{ x: number; y: number } | null>(null);
  const [level, setLevel] = useState(0);
  const [text, setText] = useState("");
  const shapeRef = useRef<HTMLDivElement>(null);
  const faceRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const bodyRef = useRef<HTMLDivElement>(null);
  const [bodyH, setBodyH] = useState(0);

  const refresh = useCallback(() => {
    void backend
      .islandView()
      .then(setView)
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    refresh();
    const stopIsland = backend.subscribeIsland((signal) => {
      if (signal.type === "view") setView(signal.view);
      else if (signal.type === "gaze") setGaze({ x: signal.x, y: signal.y });
      else setLevel(signal.level);
    });
    // Requests and the app's state come on the app's own channel.
    const stopApp = backend.subscribe((event) => {
      if (event.type === "state" || event.type === "approval" || event.type === "approval_closed") refresh();
    });
    return () => {
      stopIsland();
      stopApp();
    };
  }, [refresh]);

  const busy = Boolean(view && (view.hold || view.sending || view.message || view.notice));
  const now = useClock(busy);

  const lang = view?.lang ?? "en";
  const s = stringsFor(lang);
  const t = s.island.w;

  const state: FaceState = view ? faceStateOf(view, now) : "idle";
  const summaryShown = Boolean(view?.message && (view.message.stage === "done" || view.message.stage === "error") && now - view.message.at_ms < SUMMARY_FOR);
  const noticeShown = Boolean(view?.notice && now - view.notice.at_ms < NOTICE_FOR);
  const open = Boolean(view && (view.hover || view.approval || view.text_open || view.hold || summaryShown || noticeShown));

  const opening = useOpening(open);
  const settled = opening === 0 || opening === 1;

  // The line of text takes the keyboard when it opens.
  useEffect(() => {
    if (view?.text_open) {
      setText("");
      window.setTimeout(() => inputRef.current?.focus(), 30);
    }
  }, [view?.text_open]);

  // The body's height, for the shape's own size (it animates).
  useLayoutEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const measure = () => setBodyH(el.scrollHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [open]);

  // Tell the shell where the shape and the face are, whenever they move
  // (after every render, and at the end of each size animation).
  const last = useRef("");
  const report = () => {
    const shape = shapeRef.current?.getBoundingClientRect();
    const face = faceRef.current?.getBoundingClientRect();
    // Mid-growth the face is neither here nor there: report where it ends.
    if (!shape || !face || !settled) return;
    const key = [shape.x, shape.y, shape.width, shape.height, face.x, face.y].map(Math.round).join(",");
    if (key === last.current) return;
    last.current = key;
    void backend.islandLayout({ x: shape.x, y: shape.y, w: shape.width, h: shape.height }, face.x + face.width / 2, face.y + face.height / 2).catch(() => undefined);
  };
  const reportRef = useRef(report);
  reportRef.current = report;
  useLayoutEffect(() => reportRef.current());
  const ready = view !== null;
  useEffect(() => {
    const el = shapeRef.current;
    if (!el) return;
    const done = () => reportRef.current();
    el.addEventListener("transitionend", done);
    return () => el.removeEventListener("transitionend", done);
  }, [ready]);

  if (!view || !view.placement) return null;

  const p = view.placement;
  const notchH = p.notch.h;
  const name = view.synth?.name ?? "";
  const width = open ? Math.min(OPEN_W, p.window.w - 8) : p.pill.w;
  const height = open ? notchH + bodyH : p.pill.h;
  const keys = view.hotkeys.voice ? shortcutText(view.hotkeys.voice, view.os) : "";

  // The line under the name.
  let status: ReactNode = null;
  let detail: ReactNode = null;
  const m = view.message;
  if (view.approval) status = t.approval;
  else if (view.paused) status = t.paused;
  else if (view.synth && !view.synth.online) status = t.offline(name);
  else if (view.hold) status = (
    <>
      {view.hold === "screen_voice" ? t.releaseShot : t.release}
      <span className="isl-timer">{mmss(now - view.hold_since_ms)}</span>
    </>
  );
  else if (view.sending) status = t.sending;
  else if (m && m.stage === "accepted") status = t.thinking;
  else if (m && m.stage === "working") status = view.driving ? t.driving : t.working;
  else if (m && m.stage === "done" && summaryShown) {
    status = t.done(t.channel[m.channel] ?? "");
    detail = m.summary ? <p className="isl-summary selectable">{m.summary}</p> : null;
  } else if (m && m.stage === "error" && summaryShown) {
    status = t.error;
    detail = m.summary ? <p className="isl-summary selectable">{m.summary}</p> : null;
  } else if (view.driving) status = t.driving;
  else status = keys ? t.idle(keys) : name;
  if (view.task && (view.driving || m?.stage === "working")) {
    detail = detail ?? <p className="isl-task">{view.task}</p>;
  }

  const notice = noticeShown && view.notice ? (t.notice[view.notice.code] ?? t.notice.failed!)(view.notice.detail) : null;

  const approval = view.approval;
  const quick = approval ? quickAnswer(approval, s, approval.synth ?? name) : null;

  const left = (p.window.w - width) / 2;
  // The synth's own face, or the default look of its id.
  const face = faceFor(view.synth?.id ?? "", view.face);
  const accent = face.palette.accent;
  // One face that grows from the left ear into the open island.
  // Closed: as tall as the band and a little more (the face's own box has
  // room around the eyes), about 34 px on a notch of 32.
  const small = Math.max(22, Math.min(p.pill.h + 2, 36));
  const faceSize = small + (FACE_OPEN - small) * opening;
  const faceLeft = 8 + 8 * opening;
  const faceTop = ((notchH - small) / 2) * (1 - opening) + (notchH + 2) * opening;

  return (
    <div className={`isl-root${open ? " open" : ""}`}>
      <div
        ref={shapeRef}
        className={`isl-shape state-${state}${p.real_notch ? " notch" : ""}`}
        style={{ ["--accent" as string]: accent, left, width, height, borderBottomLeftRadius: open ? 24 : Math.min(12, p.pill.h / 2), borderBottomRightRadius: open ? 24 : Math.min(12, p.pill.h / 2) }}
      >
        <div className="isl-band" style={{ height: notchH }}>
          <div className="isl-ear left" style={{ width: (p.pill.w - p.notch.w) / 2 }} />
          <div className="isl-notch" style={{ width: p.notch.w }} />
          <div className="isl-ear right" style={{ width: (p.pill.w - p.notch.w) / 2 }}>
            <Glyph state={state} sending={view.sending} />
          </div>
        </div>

        <div ref={faceRef} className="isl-face" style={{ left: faceLeft, top: faceTop, height: faceSize }}>
          <Face config={face} state={state} gaze={gaze} level={level} size={faceSize} label={name} />
        </div>

        <div ref={bodyRef} className="isl-body" aria-hidden={!open}>
          {open ? (
            <>
              <div className="isl-head" style={{ paddingLeft: faceWidth(FACE_OPEN) + 6, minHeight: FACE_OPEN }}>
                <div className="isl-text">
                  {/* A request may come from another linked synth: its name. */}
                  <div className="isl-name">{approval?.synth ?? name}</div>
                  <div className="isl-status" role="status">
                    {status}
                  </div>
                </div>
              </div>
              {detail}
              {notice ? (
                <div className="isl-notice" role="alert">
                  {notice}
                  {view.notice?.code === "mic_denied" ? (
                    <button type="button" className="isl-btn" onClick={() => void backend.islandMic()}>
                      {t.micOpen}
                    </button>
                  ) : null}
                </div>
              ) : null}

              {approval && quick ? (
                <div className="isl-approval">
                  <div className="isl-ask-title">{quick.title}</div>
                  <div className="isl-row">
                    <button type="button" className="isl-btn" onClick={() => void backend.approvalAnswer(approval.id, false, "once").finally(refresh)}>
                      {t.deny}
                    </button>
                    {quick.unknown ? null : (
                      <button type="button" className="isl-btn primary" onClick={() => void backend.approvalAnswer(approval.id, true, quick.duration, quick.hours).finally(refresh)}>
                        {t.allow}
                      </button>
                    )}
                  </div>
                </div>
              ) : null}

              {view.text_open ? (
                <div className="isl-input">
                  <input
                    ref={inputRef}
                    value={text}
                    maxLength={4000}
                    placeholder={t.textPlaceholder(name)}
                    onChange={(e) => setText(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                        e.preventDefault();
                        void backend.islandTextSubmit(text);
                      } else if (e.key === "Escape") {
                        e.preventDefault();
                        void backend.islandTextCancel();
                      }
                    }}
                  />
                  <div className="isl-hint">{t.textHint}</div>
                </div>
              ) : null}

              {view.hover && !view.text_open && !approval ? (
                <div className="isl-row">
                  {view.synths.length > 1 ? (
                    <div className="isl-synths" role="radiogroup" aria-label={t.switchSynth}>
                      {view.synths.map((sy) => (
                        <button
                          key={sy.id}
                          type="button"
                          role="radio"
                          aria-checked={sy.id === view.synth?.id}
                          className={`isl-chip${sy.online ? "" : " off"}`}
                          onClick={() => void backend.islandSelectSynth(sy.id)}
                        >
                          {sy.name}
                        </button>
                      ))}
                    </div>
                  ) : null}
                  <span className="isl-spacer" />
                  {!view.paused && (view.driving || m?.stage === "working" || m?.stage === "accepted") ? (
                    <button type="button" className="isl-btn danger" onClick={() => void backend.islandStop()}>
                      {t.stop}
                    </button>
                  ) : null}
                  <button type="button" className="isl-btn" onClick={() => void backend.islandOpenApp()}>
                    {t.openApp}
                  </button>
                </div>
              ) : null}
            </>
          ) : null}
        </div>
      </div>
    </div>
  );
}
