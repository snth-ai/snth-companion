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
//
// With on-device speech-to-text the words of a hold show under the status
// while the key is held (small, dimmed, the last two lines), and the text
// that went stays a moment after the release. The synth's offer to turn
// it on (and then the model's download) is a card the shell sends only
// while the island is idle.
//
// Wake by name: while it listens a small dim dot sits in the closed
// island's right ear (a hollow ring while it paused by itself; hovering
// says why). The name heard, the island opens as for a hold; a click on
// it cancels the wake.
//
// Floating (Windows, `placement.floating`): no notch to merge with, so
// the closed island is a small free-standing pill with the face, every
// corner rounded, kept to the bottom edge above the taskbar
// (`placement.edge` "bottom"; "top" under a taskbar on the top edge).
// The face and a one-line row (the name and the status, or the line of
// text) stay at that edge: listening and the line of text only widen the
// pill sideways; everything else (a summary, a request, the words of a
// hold, the buttons) is a card that grows away from the edge, upwards
// at the bottom. The shape reported to the shell is the drawn shape, so
// the rest of the window stays click-through whichever way it grows.
//
// The owner's spot (Settings, `IslandPosition`): every spot but the top
// centre is this floating pill, on macOS too. The top centre is the notch
// shape on both systems (macOS: its default; Windows: when chosen, the
// drawn notch flush with the top edge, `floating` false). At the left
// or the right (`placement.align`) the shape is anchored to the pill's
// side edge and grows inwards, never past the window (which the shell
// keeps on the screen); at the right the row is mirrored (the face at the
// right end, the glyph's slot at the left), so the face stays where the
// pill is. In the middle of a side (`placement.edge` "middle") the shape
// is centred on the pill vertically (kept inside the window), the row at
// its top, the card under it, and the face keeps to the pill's height in
// a column at the pill's side (the card makes room for it).

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { quickAnswer } from "../approval/ApprovalView";
import { backend } from "../backend";
import type { IslandView, SttCard, WakeStatus } from "../backend/types";
import { stringsFor } from "../i18n";
import { shortcutText } from "../lib/format";
import { Face, faceFor, faceWidth, type FaceState } from "./faceApi";
import { aboutMb, sttLoadingVisible, tailOf, transcriptVisible } from "./sttView";

/** How long the summary stays after the answer (TZ: 4-6 s). */
const SUMMARY_FOR = 6000;
const NOTICE_FOR = 6000;
/** The open island. */
const OPEN_W = 400;
/** The face in the open island: big enough for the whole head. */
const FACE_OPEN = 84;
/** The shell's gaze reach (src-tauri island/mod.rs GAZE_REACH): the gaze
 * vector is (pointer - face centre) / GAZE_REACH, unclamped within it. */
const GAZE_REACH = 600;
/** "At the island" for the face's happy squint: on the shape, or within
 * this many px around it and below it. In gaze units 10 px is 0.017 and
 * 48 px 0.08 (the lab's distance test was a 0.22 circle, 132 px, which
 * from the left ear reaches well into the menu bar). */
const NEAR_AROUND = 10;
const NEAR_BELOW = 48;

/** Is the pointer on the island's shape or just below it (above it, for
 * an island at the bottom edge)? Rebuilt from the gaze vector and the
 * face centre the shell measures it from. */
function pointerOnIsland(g: { x: number; y: number }, shape: HTMLElement, face: HTMLElement, atBottom = false): boolean {
  if (Math.hypot(g.x, g.y) >= 0.999) return false; // clamped: farther than the reach
  const box = shape.getBoundingClientRect();
  const f = face.getBoundingClientRect();
  const px = f.x + f.width / 2 + g.x * GAZE_REACH;
  const py = f.y + f.height / 2 + g.y * GAZE_REACH;
  const above = atBottom ? NEAR_BELOW : NEAR_AROUND;
  const below = atBottom ? NEAR_AROUND : NEAR_BELOW;
  return px >= box.left - NEAR_AROUND && px <= box.right + NEAR_AROUND && py >= box.top - above && py <= box.bottom + below;
}

/** How long the island takes to open or close: the shape and the face
 * together, on one clock and one curve (`useMotion`). */
const MOVE_MS = 420;

/** The floating island (Windows): the open row at the edge (the face and
 * one line), the width when only that row is open (listening, the line
 * of text), the width of the open card, and the slot at the closed
 * pill's right end for the status glyph or the wake dot. */
const FLOAT_ROW = 34;
const FLOAT_WIDE_W = 300;
const FLOAT_CARD_W = 360;
const FLOAT_SLOT = 22;

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

/** Does the right ear (`Glyph`) show anything in this state? */
function glyphShows(state: FaceState, sending: boolean): boolean {
  switch (state) {
    case "listening":
    case "approval":
    case "error":
    case "driving":
      return false;
    case "accepted":
    case "done":
    case "thinking":
    case "working":
    case "offline":
      return true;
    default:
      return sending;
  }
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
 * `moving` is false once everything has arrived.
 *
 * Floating only (`floating`): `openW` (the open width: the row alone or
 * the card) follows the body's rule, and `closedW` (the closed width:
 * the pill makes room for its glyph) slides once the pill was placed.
 * For the notch both are constants of the placement and are taken as
 * they come, the way they always were. */
function useMotion(
  open: boolean,
  bodyH: number,
  openW: number,
  closedW: number,
  floating: boolean,
): { open: number; body: number; openW: number; closedW: number; moving: boolean } {
  const target = open ? 1 : 0;
  const tw = useRef<{ open: Tween; body: Tween; openW: Tween; closedW: Tween } | null>(null);
  if (tw.current === null) tw.current = { open: still(target), body: still(bodyH), openW: still(openW), closedW: still(closedW) };
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
    if (open && c.openW.to !== openW) {
      const settledOpen = c.open.to === 1 && !moving(c.open, t);
      c.openW = reduce || !settledOpen || !floating ? still(openW) : { from: valueAt(c.openW, t), to: openW, start: t, ms: MOVE_MS };
      changed = true;
    }
    if (c.closedW.to !== closedW) {
      c.closedW = reduce || !floating || c.closedW.to <= 0 ? still(closedW) : { from: valueAt(c.closedW, t), to: closedW, start: t, ms: MOVE_MS };
      changed = true;
    }
    if (changed) setNow(t);
  }, [target, open, bodyH, openW, closedW, floating]);

  const c = tw.current;
  const busy = moving(c.open, now) || moving(c.body, now) || moving(c.openW, now) || moving(c.closedW, now);
  // A frame after a frame while anything moves.
  useEffect(() => {
    if (!busy) return;
    const id = requestAnimationFrame(() => setNow(performance.now()));
    return () => cancelAnimationFrame(id);
  });
  return { open: valueAt(c.open, now), body: valueAt(c.body, now), openW: valueAt(c.openW, now), closedW: valueAt(c.closedW, now), moving: busy };
}

function SttCardView(props: { card: SttCard; t: ReturnType<typeof stringsFor>["island"]["w"] }) {
  const { card, t } = props;
  if (card.kind === "offer") {
    return (
      <div className="isl-stt" role="dialog" aria-label={t.sttOn}>
        <p className="isl-stt-text">{t.sttOffer(aboutMb(card.size))}</p>
        <div className="isl-row">
          <span className="isl-spacer" />
          <button type="button" className="isl-btn" onClick={() => void backend.islandSttOffer(false)}>
            {t.sttNotNow}
          </button>
          <button type="button" className="isl-btn primary" onClick={() => void backend.islandSttOffer(true)}>
            {t.sttOn}
          </button>
        </div>
      </div>
    );
  }
  if (card.kind === "download" || card.kind === "verifying") {
    const pct = card.kind === "download" ? card.percent : 100;
    return (
      <div className="isl-stt" role="status">
        <p className="isl-stt-text">{card.kind === "download" ? t.sttDownloading(card.percent) : t.sttVerifying}</p>
        <div className="isl-progress" aria-hidden="true">
          <i style={{ width: `${pct}%` }} />
        </div>
        <div className="isl-row">
          <span className="isl-spacer" />
          <button type="button" className="isl-btn" onClick={() => void backend.islandSttCardHide()}>
            {t.sttHide}
          </button>
        </div>
      </div>
    );
  }
  if (card.kind === "ready") {
    return (
      <div className="isl-stt" role="status">
        <p className="isl-stt-text">{t.sttReady}</p>
      </div>
    );
  }
  return (
    <div className="isl-stt" role="alert">
      <p className="isl-stt-text">{t.sttFailed[card.code] ?? t.sttFailed.failed}</p>
      <div className="isl-row">
        <span className="isl-spacer" />
        <button type="button" className="isl-btn" onClick={() => void backend.islandSttCardHide()}>
          {t.sttHide}
        </button>
        {card.code === "no_space" ? null : (
          <button type="button" className="isl-btn primary" onClick={() => void backend.islandSttDownload().catch(() => undefined)}>
            {t.sttRetry}
          </button>
        )}
      </div>
    </div>
  );
}

function mmss(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}

export function IslandApp() {
  const [view, setView] = useState<IslandView | null>(null);
  const [gaze, setGaze] = useState<{ x: number; y: number } | null>(null);
  /** The gaze follows the synth's own pointer (it drives the computer). */
  const [gazeAgent, setGazeAgent] = useState(false);
  /** The owner's pointer is on the island or just below it. */
  const [pointerOn, setPointerOn] = useState(false);
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
      else if (signal.type === "gaze") {
        setGaze({ x: signal.x, y: signal.y });
        setGazeAgent(signal.agent);
      }
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
  /** The owner is busy with the island: the face does not get bored then. */
  const engaged = Boolean(view && (view.hover || view.text_open || view.hold || view.approval || summaryShown));
  const noticeShown = Boolean(view?.notice && now - view.notice.at_ms < NOTICE_FOR);
  // The words of the hold (on-device speech-to-text): while the key is
  // held, while the text goes, and a moment after it was accepted.
  const transcript = view?.transcript?.trim() ?? "";
  const transcriptShown = Boolean(view && transcriptVisible(view, now));
  const sttLoadingShown = Boolean(view && sttLoadingVisible(view));
  const card = view?.stt_card ?? null;
  const open = Boolean(
    view && (view.hover || view.approval || view.text_open || view.hold || summaryShown || noticeShown || transcriptShown || card),
  );

  // Floating (the Windows default, and every spot but the top centre): the pill keeps to an
  // edge and grows away from it, from its side at the left and the right.
  const placed = view?.placement ?? null;
  const floating = Boolean(placed?.floating);
  const atBottom = floating && placed?.edge === "bottom";
  const middle = floating && placed?.edge === "middle";
  const align = floating ? (placed?.align ?? "center") : "center";
  const winW = placed?.window.w ?? 480;
  const pillW = placed?.pill.w ?? 0;
  // The width the open island may take from the pill's side inwards.
  const room =
    align === "left" ? winW - (placed?.pill.x ?? 0) - 4 : align === "right" ? (placed?.pill.x ?? 0) + pillW - 4 : winW - 8;
  const wakeNow: WakeStatus = view?.wake ?? { state: "off" };
  const wakeDot = state === "idle" && !open && (wakeNow.state === "listening" || wakeNow.state === "paused");
  const slot = floating && (glyphShows(state, Boolean(view?.sending)) || wakeDot);
  // Open: only the row (listening, the line of text) widens sideways; a
  // card (anything in the body) is wider.
  const openWTarget = floating ? Math.min(bodyH > 0 ? FLOAT_CARD_W : FLOAT_WIDE_W, room) : Math.min(OPEN_W, winW - 8);
  const closedWTarget = pillW + (slot ? FLOAT_SLOT : 0);
  const motion = useMotion(open, bodyH, openWTarget, closedWTarget, floating);
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

  // The pointer on the island (the face's happy squint), after every render:
  // the gaze moved, or the shape grew or shrank under a resting pointer.
  useLayoutEffect(() => {
    const shape = shapeRef.current;
    const faceEl = faceRef.current;
    setPointerOn(Boolean(gaze && !gazeAgent && shape && faceEl && pointerOnIsland(gaze, shape, faceEl, atBottom)));
  });

  if (!view || !view.placement) return null;

  const p = view.placement;
  const notchH = p.notch.h;
  const name = view.synth?.name ?? "";
  // The shape, from the one motion: closed is the pill, open is the
  // wide island as tall as its body.
  const k = motion.open;
  const openW = motion.openW;
  const width = p.pill.w + (openW - p.pill.w) * k;
  const height = p.pill.h + (notchH + motion.body - p.pill.h) * k;
  const closedR = Math.min(12, p.pill.h / 2);
  const radius = closedR + (24 - closedR) * Math.min(1, Math.max(0, k));
  const keys = view.hotkeys.voice ? shortcutText(view.hotkeys.voice, view.os) : "";
  const wake: WakeStatus = view.wake ?? { state: "off" };
  const woke = Boolean(view.hold && wake.state === "woke");

  // The line under the name.
  let status: ReactNode = null;
  let detail: ReactNode = null;
  const m = view.message;
  if (view.approval) status = t.approval;
  else if (view.paused) status = t.paused;
  else if (view.synth && !view.synth.online) status = t.offline(name);
  else if (view.hold) status = (
    <>
      {woke ? t.wakeListening : view.hold === "screen_voice" ? t.releaseShot : t.release}
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
  else if (wake.state === "listening" && name) status = t.idleWake(keys, name);
  else status = keys ? t.idle(keys) : name;
  const wakeNote = view.hover && wake.state === "paused" ? t.wakePaused[wake.reason] ?? null : null;
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

  // The parts both layouts show, the same elements in both.
  const wakeDotNode = wakeDot ? (
    <span
      className={`isl-wake${wake.state === "paused" ? " paused" : ""}`}
      title={wake.state === "paused" ? t.wakePaused[wake.reason] : t.wakeHint(name)}
      aria-label={wake.state === "paused" ? t.wakePaused[wake.reason] : t.wakeHint(name)}
    />
  ) : null;
  const wakeNoteNode = wakeNote ? <div className="isl-wake-note">{wakeNote}</div> : null;
  const liveNode = transcriptShown ? (
    <div className={`isl-live${view.hold ? "" : " sent"}`} aria-live="polite">
      {tailOf(transcript)}
    </div>
  ) : sttLoadingShown ? (
    <div className="isl-live loading">{t.sttLoading}</div>
  ) : null;
  const noticeNode = notice ? (
    <div className="isl-notice" role="alert">
      {notice}
      {view.notice?.code === "mic_denied" ? (
        <button type="button" className="isl-btn" onClick={() => void backend.islandMic()}>
          {t.micOpen}
        </button>
      ) : null}
    </div>
  ) : null;
  const cardNode = card && !approval ? <SttCardView card={card} t={t} /> : null;
  const approvalNode =
    approval && quick ? (
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
    ) : null;
  const inputNode = view.text_open ? (
    <input
      ref={inputRef}
      value={text}
      maxLength={4000}
      placeholder={t.textPlaceholder(name)}
      title={floating ? t.textHint : undefined}
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
  ) : null;
  const buttonsNode =
    view.hover && !view.text_open && !approval ? (
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
    ) : null;
  const faceNode = (size: number) => (
    <Face
      config={face}
      state={state}
      gaze={gaze}
      level={level}
      size={size}
      label={name}
      attention={{ near: pointerOn, engaged, agent: gazeAgent }}
      paused={view.hidden}
    />
  );

  if (floating) {
    // A free-standing pill at the edge (see the notes at the top): the
    // row with the face stays at the edge, the card grows away from it.
    const closedH = p.pill.h;
    const mirror = align === "right";
    const rowH = closedH + (FLOAT_ROW - closedH) * k;
    const fWidth = motion.closedW + (openW - motion.closedW) * k;
    // Anchored to the pill's side edge (left, right) or centred on it.
    const anchored = align === "left" ? p.pill.x : align === "right" ? p.pill.x + p.pill.w - fWidth : p.pill.x + p.pill.w / 2 - fWidth / 2;
    const fLeft = Math.max(0, Math.min(p.window.w - fWidth, anchored));
    const pillMid = p.pill.y + p.pill.h / 2;
    let fHeight: number;
    let fTop: number;
    if (middle) {
      // Centred on the pill, kept inside the window.
      fHeight = Math.max(0, Math.min(p.window.h - 4, rowH + motion.body * k));
      fTop = Math.max(2, Math.min(p.window.h - 2 - fHeight, pillMid - fHeight / 2));
    } else {
      const space = atBottom ? p.pill.y + p.pill.h : p.window.h - p.pill.y;
      fHeight = Math.max(0, Math.min(space - 2, rowH + motion.body * k));
      fTop = atBottom ? p.pill.y + p.pill.h - fHeight : p.pill.y;
    }
    const rowTop = atBottom ? fHeight - rowH : 0;
    const fRadius = Math.max(0, Math.min(fHeight / 2, 18));
    const fSmall = Math.max(22, Math.min(closedH + 2, 36));
    const fFaceSize = fSmall + (FLOAT_ROW + 2 - fSmall) * k;
    const fFaceW = faceWidth(fFaceSize);
    // Closed: centred in the pill (beside the glyph's slot); open: at the
    // row's start, the pill's side (the right end when mirrored).
    const closedFaceLeft = (p.pill.w - faceWidth(fSmall)) / 2;
    const faceInset = closedFaceLeft + (8 - closedFaceLeft) * k;
    const fFaceLeft = mirror ? fWidth - fFaceW - faceInset : faceInset;
    // In the middle the face keeps to the pill's height; elsewhere it is
    // in the row.
    const fFaceTop = middle
      ? Math.max(0, Math.min(fHeight - fFaceSize, pillMid - fTop - fFaceSize / 2))
      : rowTop + (rowH - fFaceSize) / 2;
    // The row beside the face, the slot at the other end.
    const besideFace = faceInset + fFaceW + 4;
    const slotSide = slot ? FLOAT_SLOT + 4 : 12;
    const rowSides = mirror ? { left: slotSide, right: besideFace } : { left: besideFace, right: slotSide };
    const cardW = Math.min(FLOAT_CARD_W, room);
    const cardLeft = align === "left" ? 0 : align === "right" ? fWidth - cardW : (fWidth - cardW) / 2;
    // The middle's face column: the card keeps clear of it.
    const padSide = !middle ? undefined : mirror ? { paddingRight: fFaceW + 12 } : { paddingLeft: fFaceW + 12 };
    const hasBody = open && [liveNode, wakeNoteNode, detail, noticeNode, cardNode, approvalNode, buttonsNode].some(Boolean);
    // Out of a full-screen app's way: off the window, past the edge it
    // keeps to (sideways in the middle of a side).
    const hideY = middle ? 0 : atBottom ? p.window.h - p.pill.y + 8 : -(p.pill.y + p.pill.h + 8);
    const hideX = !middle ? 0 : mirror ? p.window.w - p.pill.x + 8 : -(p.pill.x + p.pill.w + 8);

    return (
      <div
        ref={rootRef}
        className={`isl-root${open ? " open" : ""}${view.hidden ? " hidden" : ""}${middle ? " isl-side" : ""}`}
        style={middle ? { ["--hide-x" as string]: `${hideX}px` } : { ["--hide-y" as string]: `${hideY}px` }}
        aria-hidden={view.hidden}
      >
        <div
          ref={shapeRef}
          className={`isl-shape float ${middle ? "edge-middle" : atBottom ? "edge-bottom" : "edge-top"} align-${align} state-${state}`}
          style={{ ["--accent" as string]: accent, left: fLeft, top: fTop, width: fWidth, height: fHeight, borderRadius: fRadius }}
          onClick={woke ? () => void backend.islandWakeCancel() : undefined}
        >
          {/* The card shows as far as the shape has grown, from the row out. */}
          <div className={`isl-clip isl-fclip${atBottom ? " up" : ""}`} style={{ top: atBottom ? 0 : rowH, height: Math.max(0, fHeight - rowH) }}>
            {/* Laid out at the card's own width whatever the shape's width
                on the way, so its height is measured once, not as the
                text rewraps while the pill widens. */}
            <div ref={bodyRef} className="isl-fbody" style={{ width: cardW, marginLeft: cardLeft }} aria-hidden={!open}>
              {hasBody ? (
                <div className="isl-fpad" style={padSide}>
                  {liveNode}
                  {wakeNoteNode}
                  {detail}
                  {noticeNode}
                  {cardNode}
                  {approvalNode}
                  {buttonsNode}
                </div>
              ) : null}
            </div>
          </div>

          <div ref={faceRef} className="isl-face" style={{ left: fFaceLeft, top: fFaceTop, height: fFaceSize }}>
            {faceNode(fFaceSize)}
          </div>

          {open ? (
            <div className={`isl-frow${mirror ? " mirror" : ""}`} style={{ top: rowTop, height: rowH, ...rowSides }}>
              {inputNode ? (
                <div className="isl-finput">{inputNode}</div>
              ) : (
                <>
                  {/* Holding: the clock first, it must not be cut off. */}
                  {view.hold ? null : <span className="isl-fname">{approval?.synth ?? name}</span>}
                  <span className="isl-fstatus" role="status">
                    {view.hold ? <span className="isl-timer">{mmss(now - view.hold_since_ms)}</span> : null}
                    {view.hold ? (woke ? t.wakeListening : view.hold === "screen_voice" ? t.releaseShot : t.release) : status}
                  </span>
                </>
              )}
            </div>
          ) : null}

          {slot ? (
            <div className={`isl-fslot${mirror ? " mirror" : ""}`} style={{ top: rowTop, height: rowH, width: FLOAT_SLOT }}>
              <Glyph state={state} sending={view.sending} />
              {wakeDotNode}
            </div>
          ) : null}
        </div>
      </div>
    );
  }

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
        onClick={woke ? () => void backend.islandWakeCancel() : undefined}
      >
        <div className="isl-band" style={{ height: notchH }}>
          <div className="isl-ear left" style={{ width: (p.pill.w - p.notch.w) / 2 }} />
          <div className="isl-notch" style={{ width: p.notch.w }} />
          <div className="isl-ear right" style={{ width: (p.pill.w - p.notch.w) / 2 }}>
            <Glyph state={state} sending={view.sending} />
            {wakeDotNode}
          </div>
        </div>

        <div ref={faceRef} className="isl-face" style={{ left: faceLeft, top: faceTop, height: faceSize }}>
          {faceNode(faceSize)}
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
                    {wakeNoteNode}
                    {liveNode}
                  </div>
                </div>
                {detail}
                {noticeNode}

                {cardNode}

                {approvalNode}

                {inputNode ? (
                  <div className="isl-input">
                    {inputNode}
                    <div className="isl-hint">{t.textHint}</div>
                  </div>
                ) : null}

                {buttonsNode}
              </>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}
