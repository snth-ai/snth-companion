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
//
// In full screen the shell hides the island (`view.hidden`) until the menu
// bar comes in: the whole island fades out and slides up, and back, as the
// bar does (about 0.2 s).

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
/** How long the island takes to open or close: the shape and the face
 * together, on one clock and one curve (`useMotion`). */
const MOVE_MS = 420;

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

/** A CSS cubic-bezier timing function, in JS: progress 0..1 in, eased
 * value out (past 1 where the curve overshoots). */
function cubicBezier(x1: number, y1: number, x2: number, y2: number): (k: number) => number {
  const cx = 3 * x1;
  const bx = 3 * (x2 - x1) - cx;
  const ax = 1 - cx - bx;
  const cy = 3 * y1;
  const by = 3 * (y2 - y1) - cy;
  const ay = 1 - cy - by;
  const x = (t: number) => ((ax * t + bx) * t + cx) * t;
  const dx = (t: number) => (3 * ax * t + 2 * bx) * t + cx;
  const y = (t: number) => ((ay * t + by) * t + cy) * t;
  return (k: number) => {
    if (k <= 0) return 0;
    if (k >= 1) return 1;
    // The curve's t for this progress: Newton, then halving if it stalls.
    let t = k;
    for (let i = 0; i < 8; i++) {
      const err = x(t) - k;
      if (Math.abs(err) < 1e-6) return y(t);
      const d = dx(t);
      if (Math.abs(d) < 1e-6) break;
      t -= err / d;
    }
    let lo = 0;
    let hi = 1;
    t = k;
    for (let i = 0; i < 30; i++) {
      const v = x(t);
      if (Math.abs(v - k) < 1e-6) break;
      if (v < k) lo = t;
      else hi = t;
      t = (lo + hi) / 2;
    }
    return y(t);
  };
}

/** The island's spring: it overshoots a little and settles (what the
 * shape's CSS transition used to be). */
const SPRING = cubicBezier(0.22, 1.28, 0.36, 1);

/** One value on its way from `from` to `to`, started at `start`
 * (performance.now()). */
interface Tween {
  from: number;
  to: number;
  start: number;
  ms: number;
}

const still = (v: number): Tween => ({ from: v, to: v, start: 0, ms: 0 });
const moving = (tw: Tween, now: number) => tw.ms > 0 && now < tw.start + tw.ms;
function valueAt(tw: Tween, now: number): number {
  if (!moving(tw, now)) return tw.to;
  return tw.from + (tw.to - tw.from) * SPRING(Math.max(0, (now - tw.start) / tw.ms));
}

function reducedMotion(): boolean {
  return typeof matchMedia === "function" && matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** The island's motion on one clock: `open` 0 (closed) .. 1 (open), a
 * little past either end in the spring's overshoot, and the body's height
 * as the shape shows it. Every size on the island (the shape's width,
 * height and corners, the face's size and place) is read from these in
 * the same frame, so the face never runs ahead of the shape or lags it.
 * `moving` is false once everything has arrived. */
function useMotion(open: boolean, bodyH: number): { open: number; body: number; moving: boolean } {
  const target = open ? 1 : 0;
  const tw = useRef<{ open: Tween; body: Tween } | null>(null);
  if (tw.current === null) tw.current = { open: still(target), body: still(bodyH) };
  const [now, setNow] = useState(() => performance.now());

  // New targets, before the frame is drawn: from wherever the island is.
  useLayoutEffect(() => {
    const c = tw.current!;
    const t = performance.now();
    const reduce = reducedMotion();
    let changed = false;
    if (c.open.to !== target) {
      const from = valueAt(c.open, t);
      // Turned back mid-way (the pointer left while it opened): the way
      // back takes as much of the time as there is way to go.
      c.open = reduce ? still(target) : { from, to: target, start: t, ms: MOVE_MS * Math.min(1, Math.max(0.4, Math.abs(target - from))) };
      changed = true;
    }
    // The body counts only while open. Opening, the shape heads for the
    // body as it is (measured before the first frame); open, the body's
    // own changes (a summary arrives) slide on the same curve; closing,
    // the shape shrinks from the body it had.
    if (open && c.body.to !== bodyH) {
      const settledOpen = c.open.to === 1 && !moving(c.open, t);
      c.body = reduce || !settledOpen ? still(bodyH) : { from: valueAt(c.body, t), to: bodyH, start: t, ms: MOVE_MS };
      changed = true;
    }
    if (changed) setNow(t);
  }, [target, open, bodyH]);

  const c = tw.current;
  const busy = moving(c.open, now) || moving(c.body, now);
  // A frame after a frame while anything moves.
  useEffect(() => {
    if (!busy) return;
    const id = requestAnimationFrame(() => setNow(performance.now()));
    return () => cancelAnimationFrame(id);
  });
  return { open: valueAt(c.open, now), body: valueAt(c.body, now), moving: busy };
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
  const rootRef = useRef<HTMLDivElement>(null);
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

  const motion = useMotion(open, bodyH);
  const settled = !motion.moving;

  // The line of text takes the keyboard when it opens.
  useEffect(() => {
    if (view?.text_open) {
      setText("");
      window.setTimeout(() => inputRef.current?.focus(), 30);
    }
  }, [view?.text_open]);

  // The body's height, for the shape's own size (it moves with the rest).
  useLayoutEffect(() => {
    const el = bodyRef.current;
    if (!el) return;
    const measure = () => setBodyH(el.scrollHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [open]);

  // Tell the shell where the shape and the face are once they have
  // arrived (after every render; the last frame of a move is one).
  const last = useRef("");
  const report = () => {
    const base = rootRef.current?.getBoundingClientRect();
    const box = shapeRef.current?.getBoundingClientRect();
    const fbox = faceRef.current?.getBoundingClientRect();
    // Mid-growth the face is neither here nor there: report where it ends.
    if (!base || !box || !fbox || !settled) return;
    // Against the root, which sits at the window's corner: the slide in
    // and out of full screen moves the root, not the island in it.
    const shape = { x: box.x - base.x, y: box.y - base.y, w: box.width, h: box.height };
    const face = { x: fbox.x - base.x + fbox.width / 2, y: fbox.y - base.y + fbox.height / 2 };
    const key = [shape.x, shape.y, shape.w, shape.h, face.x, face.y].map(Math.round).join(",");
    if (key === last.current) return;
    last.current = key;
    void backend.islandLayout(shape, face.x, face.y).catch(() => undefined);
  };
  const reportRef = useRef(report);
  reportRef.current = report;
  useLayoutEffect(() => reportRef.current());

  if (!view || !view.placement) return null;

  const p = view.placement;
  const notchH = p.notch.h;
  const name = view.synth?.name ?? "";
  // The shape, from the one motion: closed is the pill, open is the
  // wide island as tall as its body.
  const k = motion.open;
  const openW = Math.min(OPEN_W, p.window.w - 8);
  const width = p.pill.w + (openW - p.pill.w) * k;
  const height = p.pill.h + (notchH + motion.body - p.pill.h) * k;
  const closedR = Math.min(12, p.pill.h / 2);
  const radius = closedR + (24 - closedR) * Math.min(1, Math.max(0, k));
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
  // The same `k` as the shape's, in the same frame (the overshoot too).
  const faceSize = small + (FACE_OPEN - small) * k;
  const faceLeft = 8 + 8 * k;
  const faceTop = ((notchH - small) / 2) * (1 - k) + (notchH + 2) * k;

  return (
    <div
      ref={rootRef}
      className={`isl-root${open ? " open" : ""}${view.hidden ? " hidden" : ""}`}
      style={{ ["--hide-y" as string]: `${-(p.pill.h + 8)}px` }}
      aria-hidden={view.hidden}
    >
      <div
        ref={shapeRef}
        className={`isl-shape state-${state}${p.real_notch ? " notch" : ""}`}
        style={{ ["--accent" as string]: accent, left, width, height, borderBottomLeftRadius: radius, borderBottomRightRadius: radius }}
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

        {/* The body shows as far as the shape has grown. */}
        <div className="isl-clip" style={{ height: Math.max(0, height - notchH) }}>
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
    </div>
  );
}
