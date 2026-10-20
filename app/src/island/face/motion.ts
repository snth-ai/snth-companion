// How the face moves. A pose is a set of named numbers (where the eyes look,
// how far each lid is closed, brows, mouth, head). Each state and emote
// says where the pose should go (`targetPose`); springs carry the pose
// there. Blinks are a fast curve laid on top of the lids after the springs,
// so they never wobble.
//
// Energy: FaceSim runs requestAnimationFrame only while something moves.
// When every spring has settled and no timeline is running it stops and
// sets one timer for the next scheduled thing (blink, glance, emote,
// falling asleep). States that keep moving on their own (sleeping, working,
// thinking, listening) run at a capped frame rate on a timer.
//
// Reduced motion: every state has one fixed pose, a state change is a short
// cross-fade, nothing blinks or wanders; only the microphone meter moves.

// The .ts extension keeps this file runnable as is under `node --test`.
import { makeRig, type MotionStyle, type Rig, type RigFrame, type SmoothTuning } from "./rig.ts";
import type { FaceConfig, FaceState, IdleEmote } from "./schema";

// ---------- the clock ----------
//
// Every face reads time from here. The rate is 1 except in the face lab,
// which slows it down (0.25x) to inspect motion; changing it rebases, so
// time never jumps.

let clockScale = 1;
let clockRealBase = 0;
let clockVirtBase = 0;

/** Face time in seconds for a performance.now() / rAF timestamp (ms). */
export function faceClock(realMs: number = performance.now()): number {
  return (clockVirtBase + (realMs - clockRealBase) * clockScale) / 1000;
}

/** Face time runs at this rate (1 = real time). */
export function setFaceTimeScale(scale: number): void {
  const now = performance.now();
  clockVirtBase = clockVirtBase + (now - clockRealBase) * clockScale;
  clockRealBase = now;
  clockScale = Math.max(0.01, scale);
}

export function faceTimeScale(): number {
  return clockScale;
}

/** How far lids.rest = 1 closes the eye (a sliver always shows). */
export const LID_REST = 0.58;

export const CHANNELS = [
  "gx", // where the irises look, -1..1
  "gy",
  "hx", // head offset, face units
  "hy",
  "hr", // head tilt, degrees
  "uL", // upper lid closure 0..1, left / right
  "uR",
  "lL", // lower lid closure 0..1
  "lR",
  "tL", // lid tilt: + inner corner lower (focus, anger), - outer lower (sad)
  "tR",
  "sL", // eye scale
  "sR",
  "eyL", // eye lifted (-) or lowered (+), in eye radii
  "eyR",
  "lowCurve", // lower lid edge: -1 hugs the eye, +1 bows up (smiling eyes)
  "closedCurve", // a closed eye's line: -1 relaxed (u), +1 happy (^)
  "pupil", // pupil dilation, 1 = as configured
  "bLl", // brow lift (-1..1) and angle (degrees, + inner end raised)
  "bLa",
  "bRl",
  "bRa",
  "smile", // -1 frown .. 1 grin
  "open", // mouth open 0..1
  "mouthX", // mouth shift sideways, -1..1
  "blush", // added to cheeks.blush
  "heart", // heart pupils 0..1
  "dim", // colours drained 0..1
  "off", // offline: eyes become two grey capsules 0..1
  "detail", // brows, mouth, accessories shown 0..1 (follows the size)
  "ear", // accessory twitch (degrees)
  "gl", // glasses lift: + pushed up, - slid down the nose (in lens heights)
  "glr", // glasses tilt, degrees
  "ax", // trailing copy of the head for antenna and halo (lags behind)
  "ay",
  "stretch", // pill eyes: taller (+) or squashed (-), as a fraction of the height
] as const;

export type Channel = (typeof CHANNELS)[number];
export type Pose = Record<Channel, number>;

type Group = "gaze" | "head" | "face" | "slow" | "pop" | "wobble" | "fast";
const GROUP: Record<Channel, Group> = {
  gx: "gaze",
  gy: "gaze",
  hx: "head",
  hy: "head",
  hr: "head",
  uL: "face",
  uR: "face",
  lL: "face",
  lR: "face",
  tL: "face",
  tR: "face",
  sL: "pop",
  sR: "pop",
  eyL: "face",
  eyR: "face",
  lowCurve: "face",
  closedCurve: "face",
  pupil: "face",
  bLl: "face",
  bLa: "face",
  bRl: "face",
  bRa: "face",
  smile: "face",
  open: "face",
  mouthX: "face",
  blush: "slow",
  heart: "pop",
  dim: "slow",
  off: "fast",
  detail: "fast",
  ear: "wobble",
  gl: "pop",
  glr: "face",
  ax: "wobble",
  ay: "wobble",
  stretch: "pop",
};

/** When a channel counts as settled, in its own units: well under what a
 * pixel shows, so the loop stops as soon as nothing visible moves. */
const EPS: Partial<Record<Channel, number>> = {
  hx: 0.04,
  hy: 0.04,
  hr: 0.08,
  ax: 0.08,
  ay: 0.08,
  ear: 0.3,
  bLa: 0.15,
  bRa: 0.15,
  glr: 0.15,
  blush: 0.01,
  dim: 0.01,
  off: 0.01,
  detail: 0.01,
};

export interface Vec {
  x: number;
  y: number;
}

/** Everything the drawing needs for one frame. */
export interface Frame {
  pose: Pose;
  /** Blink closure 0..1 for each eye, applied over the lids. */
  blinkL: number;
  blinkR: number;
  /** Smoothed microphone level 0..1, and a slower copy. */
  level: number;
  levelSlow: number;
  /** The state being shown (idle may become sleeping by itself). */
  state: FaceState;
  /** Seconds since that state began, and a running clock. */
  st: number;
  t: number;
  /** Reduced motion: the previous state's frame, fading out (a goes 0 to 1). */
  fade?: { from: Frame; a: number };
  /** Pill eyes: the springy settle after a blink, a damped wave around 0
   * (+ = landed low and squashed). Absent for other shapes. */
  rebound?: number;
  /** Pill eyes with a motion style other than classic: the gaze rig's
   * head, breath and attention (see rig.ts). */
  rig?: RigFrame;
}

export interface EmoteRun {
  kind: IdleEmote;
  t: number; // seconds since it began
  side: 1 | -1;
}

export const EMOTE_LENGTH: Record<IdleEmote, number> = {
  wink: 0.75,
  look_around: 2.5,
  yawn: 2.7,
  heart_eyes: 2.1,
  squint: 1.7,
  surprised: 1.2,
};

/** How often each habit comes up (relative), and which ones are rare. */
const EMOTE_WEIGHT: Record<IdleEmote, number> = {
  look_around: 70,
  squint: 12.5,
  yawn: 12.5,
  wink: 5 / 3,
  heart_eyes: 5 / 3,
  surprised: 5 / 3,
};
const RARE: readonly IdleEmote[] = ["wink", "heart_eyes", "surprised"];
const RARE_COOLDOWN = 180;

/** How long the one-shot states play before they hold. */
const TIMELINE: Partial<Record<FaceState, number>> = { accepted: 0.8, done: 1.5, error: 0.6, approval: 0.45 };

/** States worth a blink when the face turns its attention to them. */
const BLINK_INTO: ReadonlySet<FaceState> = new Set(["idle", "listening", "thinking", "working", "driving"]);
const BLINK_COOLDOWN = 1.2;

/** States drawn with a symbol beside the eyes (Face.tsx). */
const SYMBOL_STATES: ReadonlySet<FaceState> = new Set(["driving", "approval", "error", "listening"]);

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
const smooth = (t: number) => {
  const x = clamp(t, 0, 1);
  return x * x * (3 - 2 * x);
};
/** 0 before a, rises to 1 by a+rise, stays, falls to 0 by b. */
function envelope(t: number, a: number, b: number, rise = 0.15, fall = 0.25): number {
  if (t <= a || t >= b) return 0;
  return Math.min(smooth((t - a) / rise), smooth((b - t) / fall));
}

/** Accepted: one small nod. Down by 120 ms, back by 350 ms, settled by 800 ms.
 * 0..1 of the nod depth (slightly negative on the rebound). Pure, so the
 * drawing applies it straight, without spring lag. */
export function nodCurve(st: number): number {
  if (st <= 0) return 0;
  if (st < 0.12) return 1 - (1 - st / 0.12) ** 2;
  if (st < 0.35) return 1 - smooth((st - 0.12) / 0.23) * 1.15;
  if (st < 0.8) return -0.15 * (1 - smooth((st - 0.35) / 0.45));
  return 0;
}

/** Approval: one lift on entry, 180 ms up, 250 ms back, then hold. */
export function liftCurve(st: number): number {
  if (st <= 0) return 0;
  if (st < 0.18) return 1 - (1 - st / 0.18) ** 2;
  return 1 - smooth((st - 0.18) / 0.25);
}

export interface TargetInput {
  face: FaceConfig;
  state: FaceState;
  st: number;
  t: number;
  /** Where the eyes are fixed (after saccades), and where the head leans. */
  look: Vec;
  lean: Vec;
  level: number;
  emote: EmoteRun | null;
  reduced: boolean;
  compact: boolean;
}

/** The pose a state wants at a moment. Pure. */
export function targetPose(input: TargetInput): Pose {
  const { face, state, look, lean, level, emote, reduced } = input;
  // Reduced motion: one fixed pose per state, nothing that runs on a clock.
  const st = reduced ? (HERO_TIME[state] ?? 1.2) : input.st;
  const t = reduced ? 0 : input.t;
  const cur = face.motion.curiosity;
  const rest = face.eyes.lids.rest * LID_REST;
  const style = face.mouth.style;
  const baseSmile = style === "smile" ? 0.55 : style === "open" ? 0.6 : style === "cat" ? 0.3 : 0.05;
  const baseOpen = style === "open" ? 0.42 : 0;
  const leanAmt = 0.35 + 0.65 * cur;

  const p: Pose = {
    gx: look.x,
    gy: look.y,
    hx: lean.x * 3.4 * leanAmt,
    hy: lean.y * 1.6 * leanAmt,
    hr: lean.x * 3.2 * leanAmt,
    uL: rest,
    uR: rest,
    lL: 0,
    lR: 0,
    tL: 0,
    tR: 0,
    sL: 1,
    sR: 1,
    eyL: 0,
    eyR: 0,
    lowCurve: -0.4,
    closedCurve: -0.5,
    pupil: 1,
    bLl: 0,
    bLa: 0,
    bRl: 0,
    bRa: 0,
    smile: baseSmile,
    open: baseOpen,
    mouthX: 0,
    blush: 0,
    heart: 0,
    dim: 0,
    off: 0,
    detail: input.compact ? 0 : 1,
    ear: 0,
    gl: 0,
    glr: 0,
    ax: 0,
    ay: 0,
    stretch: 0,
  };
  const both = (a: "u" | "l" | "t" | "s", v: number) => {
    p[`${a}L` as Channel] = v;
    p[`${a}R` as Channel] = v;
  };
  const brows = (lift: number, angle: number) => {
    p.bLl = lift;
    p.bRl = lift;
    p.bLa = angle;
    p.bRa = angle;
  };

  switch (state) {
    case "idle":
      break;

    case "sleeping": {
      const breath = Math.sin((t * Math.PI * 2) / 4.2);
      p.gx = 0;
      p.gy = 0.3;
      both("u", 0.6);
      both("l", 0.45);
      p.closedCurve = -1;
      p.hx = 0;
      p.hy = 3 + breath * 1.1;
      p.hr = 5 + breath * 0.8;
      p.smile = Math.max(0.12, baseSmile * 0.4);
      p.open = baseOpen * 0.3 + 0.06 + 0.05 * breath;
      brows(-0.25, 3);
      p.dim = 0.08;
      p.gl = -0.22;
      p.glr = 4;
      break;
    }

    case "listening": {
      // Straight ahead at the person talking; the meter shows the voice.
      p.gx = 0;
      p.gy = 0.08;
      both("u", rest * 0.25);
      both("l", 0.02);
      both("s", 1.03 + level * 0.04);
      p.pupil = 1.1 + level * 0.12;
      brows(0.22 + level * 0.3, 4);
      p.smile = Math.max(baseSmile, 0.2);
      p.open = baseOpen * 0.5;
      p.hr = 4;
      p.hx = 0;
      p.hy = -0.5;
      break;
    }

    case "accepted": {
      // A small happy nod; the nod itself is drawn from nodCurve.
      both("u", 0);
      both("l", 0.18);
      p.lowCurve = 0.5;
      brows(0.3, 2);
      p.smile = Math.max(0.7, baseSmile);
      p.open = baseOpen;
      p.blush = 0.15;
      p.hr = 2;
      p.hx = 0;
      p.hy = 0;
      p.gx = look.x * 0.3;
      p.gy = 0.15;
      p.ear = st < 0.3 ? 6 : 0;
      break;
    }

    case "thinking": {
      // Mostly open, one eye a touch higher, looking up and around.
      const spots: Vec[] = [
        { x: 0.6, y: -0.7 },
        { x: -0.35, y: -0.75 },
        { x: 0.25, y: -0.85 },
        { x: 0.7, y: -0.45 },
        { x: -0.15, y: -0.6 },
      ];
      const spot = spots[Math.floor(st / 1.05) % spots.length] ?? spots[0]!;
      p.gx = spot.x;
      p.gy = spot.y;
      p.uL = 0;
      p.uR = rest * 0.3 + 0.03;
      p.lL = 0.02;
      p.lR = 0.04;
      p.eyL = -0.1;
      p.sL = 1.03;
      p.bLl = 0.6;
      p.bLa = 4;
      p.bRl = -0.05;
      p.bRa = -6;
      p.smile = baseSmile * 0.3 - 0.08;
      p.open = 0;
      p.mouthX = 0.4;
      p.hr = -5;
      p.hx = 1.5;
      p.hy = -1;
      break;
    }

    case "working": {
      // Level, two-thirds open, short scans downward.
      const period = 1.6;
      const ph = (st % period) / period;
      p.gx = ph < 0.8 ? -0.3 + Math.floor(ph * 5) * 0.15 : -0.3;
      p.gy = 0.5;
      both("u", Math.max(rest * 0.9, 0.17));
      both("l", 0.15);
      p.lowCurve = -0.2;
      both("t", 0.05);
      brows(-0.15, -6);
      p.smile = baseSmile * 0.3;
      p.open = 0;
      p.hr = Math.sin(st * 1.15) * 1.2;
      p.hx = 0;
      p.hy = 1.2;
      break;
    }

    case "driving": {
      // Following its own cursor, focused; the cursor glyph says why.
      p.gx = look.x;
      p.gy = look.y;
      p.hx = lean.x * 4.5;
      p.hy = lean.y * 2;
      p.hr = lean.x * 4;
      both("u", rest * 0.6 + 0.1);
      both("l", 0.06);
      both("t", 0.08);
      brows(0, -8);
      p.smile = Math.max(0.2, baseSmile * 0.6);
      p.open = 0;
      p.mouthX = 0.25;
      break;
    }

    case "approval": {
      // Wide and attentive, looking straight out; one lift on entry.
      const lift = liftCurve(st);
      p.gx = 0;
      p.gy = 0.05;
      both("u", 0);
      both("l", 0.02);
      both("s", 1.08 + 0.05 * lift);
      p.pupil = 1.35;
      brows(0.5, 14);
      p.smile = 0.05;
      p.open = Math.max(0.08, baseOpen * 0.4);
      p.hr = 4;
      p.hx = 0;
      p.hy = -1 - 2 * lift;
      p.blush = 0.1;
      p.gl = 0.15;
      break;
    }

    case "done": {
      const hop = envelope(st, 0, 0.42, 0.12, 0.3);
      both("u", 0);
      both("l", 0.74);
      p.lowCurve = 1;
      p.closedCurve = 1;
      brows(0.42, 3);
      p.smile = 1;
      p.open = Math.max(0.3, baseOpen);
      p.blush = 0.3;
      p.hy = -4 * hop;
      p.hr = 3 * Math.sin(st * 8) * Math.exp(-st * 2.2) + 2;
      p.hx = 0;
      p.gx = 0;
      p.gy = 0;
      p.ear = st < 0.4 ? -8 : 0;
      p.gl = 0.1 * hop;
      break;
    }

    case "error": {
      // Concerned, not comic: slightly uneven eyes, worried brows, one small
      // head correction, then hold. The "!" glyph says what happened.
      p.gx = 0.15;
      p.gy = 0.3;
      p.sL = 1.05;
      p.sR = 0.94;
      p.uL = 0;
      p.uR = rest * 0.4 + 0.1;
      p.lR = 0.06;
      p.pupil = 0.75;
      p.bLl = 0.45;
      p.bLa = 9;
      p.bRl = 0.25;
      p.bRa = 5;
      p.smile = -0.4;
      p.open = 0.04;
      p.mouthX = -0.15;
      p.hr = 4 + (st < 0.3 ? 2 * Math.sin((st / 0.3) * Math.PI) : 0);
      p.hx = 0;
      p.hy = 0.5;
      p.glr = 5;
      p.gl = -0.08;
      break;
    }

    case "offline": {
      p.gx = 0;
      p.gy = 0.3;
      both("u", 0.3);
      p.pupil = 0.9;
      brows(-0.2, 2);
      p.smile = baseSmile * 0.2;
      p.open = 0;
      p.hx = 0;
      p.hr = 0;
      p.hy = 1.5;
      p.dim = 1;
      p.off = 1;
      break;
    }
  }

  const pill = face.eyes.shape === "pill";
  if (pill) pillPose(p, state, st, t, level, rest);

  if (state === "idle" && emote && !reduced) applyEmote(p, emote, rest, baseSmile);

  // Eyelids follow the eyes a little: looking down lowers them. Pill eyes
  // have no lids; the drawing squashes them with the gaze instead.
  if (!pill) {
    const follow = p.gy * 0.1;
    p.uL = clamp(p.uL + follow, 0, 1);
    p.uR = clamp(p.uR + follow, 0, 1);
  }

  if (reduced) {
    p.hx = 0;
    p.hy = 0;
    p.hr = state === "sleeping" ? 4 : 0;
    p.ear = 0;
  }
  p.ax = p.hx;
  p.ay = p.hy;
  return p;
}

/** Pill eyes have no lids, iris or brows to act with, so the states are
 * told by the shape of the whole eye: how tall, how squashed, how tilted
 * (tL / tR turn the eye, + inner side lower), and a closed eye is a bar
 * that closedCurve bends (+ happy arch, - relaxed). Runs after the state's
 * own pose, before the idle emotes. */
function pillPose(p: Pose, state: FaceState, st: number, t: number, level: number, rest: number): void {
  // A blink is a flat bar, not a curve.
  p.closedCurve = 0;
  switch (state) {
    case "sleeping": {
      // Flat bars low in the eye, a slow breath in their width and height.
      const breath = Math.sin((t * Math.PI * 2) / 4.2);
      p.uL = p.uR = 0.9;
      p.lL = p.lR = 0.06;
      p.sL = p.sR = 1 + 0.035 * breath;
      p.stretch = 0.15 + 0.12 * breath;
      break;
    }
    case "listening":
      // Taller and attentive; the voice makes them swell a little. Taller,
      // not bigger, so the meter beside the eye keeps its room.
      p.stretch = 0.06 + level * 0.12;
      p.uL = p.uR = 0;
      p.sL = p.sR = 1;
      break;
    case "accepted":
      // Cheeks up (the bottom rises) and a squash on the nod.
      p.lL = p.lR = 0.2;
      p.stretch = -0.14 * nodCurve(st);
      break;
    case "thinking":
      // Up and to the side; one eye squashed from the top and turned.
      p.uL = rest * 0.3;
      p.uR = 0.3;
      p.tR = 0.22;
      p.tL = -0.05;
      p.stretch = 0.03;
      break;
    case "working":
      // Two-thirds open and level, focused.
      p.tL = p.tR = 0.08;
      break;
    case "approval": {
      // Tall, a pleading turn (outer sides lower), a stretch on the lift.
      const lift = liftCurve(st);
      p.stretch = 0.1 + 0.08 * lift;
      p.sL = p.sR = 1;
      p.tL = p.tR = -0.28;
      break;
    }
    case "done": {
      // Happy arches (^ ^) that hop with the head, and a wide thin smile
      // instead of an open mouth.
      const hop = envelope(st, 0, 0.42, 0.12, 0.3);
      p.stretch = 0.25 * hop;
      p.open = 0;
      break;
    }
    case "error": {
      // A wince on the hit, then smaller, uneven, worried eyes.
      const hit = st < 0.35 ? Math.sin((st / 0.35) * Math.PI) : 0;
      p.sL = 0.86;
      p.sR = 0.78;
      p.uL = 0.1 + 0.3 * hit;
      p.uR = 0.18 + 0.3 * hit;
      p.lL = p.lR = 0.06 + 0.15 * hit;
      p.tL = p.tR = -0.32;
      p.stretch = -0.06;
      break;
    }
    case "offline":
      p.uL = p.uR = 0.85;
      p.lL = p.lR = 0.1;
      break;
    default:
      break;
  }
}

function applyEmote(p: Pose, e: EmoteRun, rest: number, baseSmile: number): void {
  const len = EMOTE_LENGTH[e.kind];
  const t = e.t;
  switch (e.kind) {
    case "wink": {
      const k = envelope(t, 0.04, len - 0.05, 0.1, 0.22);
      const side = e.side;
      const u = side > 0 ? "uR" : "uL";
      const l = side > 0 ? "lR" : "lL";
      p[u] = lerp(p[u], 0.62, k);
      p[l] = lerp(p[l], 0.5, k);
      p.closedCurve = lerp(p.closedCurve, 0.9, k);
      p.smile = lerp(p.smile, Math.max(0.8, baseSmile), k);
      p.mouthX += side * 0.35 * k;
      if (side > 0) {
        p.bRl -= 0.2 * k;
        p.bLl += 0.25 * k;
      } else {
        p.bLl -= 0.2 * k;
        p.bRl += 0.25 * k;
      }
      p.hr += side * 4 * k;
      p.blush += 0.15 * k;
      p.gl -= 0.15 * k;
      p.glr += side * 3 * k;
      break;
    }
    case "look_around": {
      const left = envelope(t, 0.1, 1.15, 0.12, 0.12);
      const right = envelope(t, 1.15, 2.3, 0.12, 0.2);
      const x = -0.85 * left + 0.85 * right;
      p.gx = lerp(p.gx, x, Math.max(left, right));
      p.gy = lerp(p.gy, -0.05, Math.max(left, right));
      p.hx += x * 3;
      p.hr += x * 3;
      p.bLl += 0.12 * Math.max(left, right);
      p.bRl += 0.12 * Math.max(left, right);
      break;
    }
    case "yawn": {
      const k = envelope(t, 0.1, 2.3, 0.6, 0.5);
      p.uL = lerp(p.uL, 0.62, k);
      p.uR = lerp(p.uR, 0.62, k);
      p.lL = lerp(p.lL, 0.42, k);
      p.lR = lerp(p.lR, 0.42, k);
      p.closedCurve = lerp(p.closedCurve, -0.2, k);
      p.open = lerp(p.open, 0.95, k);
      p.smile = lerp(p.smile, -0.1, k);
      p.bLl += 0.4 * k;
      p.bRl += 0.4 * k;
      p.bLa += 8 * k;
      p.bRa += 8 * k;
      p.hy -= 2.2 * k;
      p.hr -= 5 * k;
      p.stretch -= 0.1 * k;
      break;
    }
    case "heart_eyes": {
      const k = envelope(t, 0.15, len - 0.1, 0.1, 0.25);
      p.heart = k;
      p.blush += 0.45 * k;
      p.smile = lerp(p.smile, 0.9, k);
      p.uL = lerp(p.uL, 0, k);
      p.uR = lerp(p.uR, 0, k);
      p.bLl += 0.3 * k;
      p.bRl += 0.3 * k;
      p.bLa += 6 * k;
      p.bRa += 6 * k;
      p.hy -= 1.5 * k;
      p.hr += 3 * k;
      break;
    }
    case "squint": {
      const k = envelope(t, 0.05, len - 0.1, 0.18, 0.3);
      const peek = envelope(t, 0.35, len - 0.2, 0.15, 0.2);
      p.uL = lerp(p.uL, rest + 0.34, k);
      p.uR = lerp(p.uR, rest + 0.34, k);
      p.lL = lerp(p.lL, 0.26, k);
      p.lR = lerp(p.lR, 0.26, k);
      p.lowCurve = lerp(p.lowCurve, 0.1, k);
      p.tL += 0.25 * k;
      p.tR += 0.25 * k;
      p.bLl -= 0.2 * k;
      p.bRl -= 0.2 * k;
      p.bLa -= 10 * k;
      p.bRa -= 10 * k;
      p.gx = lerp(p.gx, 0.45, peek);
      p.mouthX -= 0.4 * k;
      p.smile = lerp(p.smile, -0.05, k);
      p.gl -= 0.15 * k;
      break;
    }
    case "surprised": {
      const k = envelope(t, 0.02, len - 0.1, 0.06, 0.35);
      p.sL = lerp(p.sL, 1.18, k);
      p.sR = lerp(p.sR, 1.18, k);
      p.pupil = lerp(p.pupil, 0.6, k);
      p.uL = lerp(p.uL, 0, k);
      p.uR = lerp(p.uR, 0, k);
      p.lL = lerp(p.lL, 0, k);
      p.lR = lerp(p.lR, 0, k);
      p.bLl += 0.8 * k;
      p.bRl += 0.8 * k;
      p.open = lerp(p.open, 0.5, k);
      p.smile = lerp(p.smile, -0.1, k);
      p.hy -= 2 * k;
      p.stretch += 0.16 * k;
      p.ear = -14 * k;
      p.gl += 0.3 * k;
      break;
    }
  }
}

/** The head style's attention shows on the pose: a startle widens the eyes
 * (and parts the mouth), a pointer close by brings a happy squint. */
function applyRig(p: Pose, r: RigFrame): void {
  const s = r.surprise;
  if (s > 0.001) {
    p.stretch += 0.2 * s;
    p.sL += 0.07 * s;
    p.sR += 0.07 * s;
    p.open = lerp(p.open, 0.35, s);
    p.lL = lerp(p.lL, 0, s);
    p.lR = lerp(p.lR, 0, s);
  }
  // The squint is the cheeks pushing the beans up from below and the lids
  // coming down a little: shorter, a touch wider, still a bean (a full happy
  // arch reads as a cloud here).
  const q = r.squint * (1 - s);
  if (q > 0.001) {
    p.lL = lerp(p.lL, 0.26, q);
    p.lR = lerp(p.lR, 0.26, q);
    p.uL = lerp(p.uL, 0.14, q);
    p.uR = lerp(p.uR, 0.14, q);
    // Lowered, so the squint does not lift the gaze off the pointer below.
    p.eyL += 0.22 * q;
    p.eyR += 0.22 * q;
    p.lowCurve = lerp(p.lowCurve, 0.2, q);
    p.stretch -= 0.06 * q;
    p.smile = lerp(p.smile, Math.max(p.smile, 0.9), q);
  }
}

/** The moment of each state a still picture (and reduced motion) shows. */
export const HERO_TIME: Partial<Record<FaceState, number>> = {
  accepted: 0.12,
  done: 0.55,
  error: 1.0,
  approval: 1.0,
  sleeping: 2.2,
};

// ---------- the running face ----------

export interface SimInput {
  face: FaceConfig;
  state: FaceState;
  gaze: Vec | null;
  level: number;
  reduced: boolean;
  autoSleep: boolean;
  compact: boolean;
  /** Pill eyes only: how the gaze moves (rig.ts). Default classic. */
  motionStyle?: MotionStyle;
  /** Live tuning of the smooth style (the lab's sliders). */
  rigTuning?: Partial<SmoothTuning>;
}

/** The motion style in effect: the rig works on pill eyes only. */
export function styleOf(input: Pick<SimInput, "face" | "motionStyle">): MotionStyle {
  return input.face.eyes.shape === "pill" ? (input.motionStyle ?? "classic") : "classic";
}

interface Spring {
  k: number;
  c: number;
}

const spring = (k: number, zeta: number): Spring => ({ k, c: 2 * zeta * Math.sqrt(k) });

function springFor(g: Group, face: FaceConfig, state: FaceState): Spring {
  const cur = face.motion.curiosity;
  const live = face.motion.liveliness;
  switch (g) {
    case "gaze":
      // At the computer the eyes stay on the cursor: stiff, no overshoot.
      // Pill eyes move as whole eyes and land with a little give.
      if (state === "driving") return spring(1400, 1);
      return face.eyes.shape === "pill" ? spring(300 + 420 * cur, 0.62) : spring(420 + 680 * cur, 0.92);
    case "head":
      // Calm characters stop cleanly; lively ones keep a little bounce.
      return spring(70 + 70 * live, 0.65 + 0.3 * (1 - live));
    case "face":
      return spring(230, 0.82);
    case "slow":
      return spring(30, 1);
    case "pop":
      return spring(300, 0.8);
    case "wobble":
      return spring(45, 0.6);
    case "fast":
      return spring(700, 1);
  }
}

const rand = (lo: number, hi: number) => lo + Math.random() * (hi - lo);

/** The fps cap for states that keep moving by themselves (0 = none). */
function ambientFps(state: FaceState, level: number): number {
  switch (state) {
    case "sleeping":
      return 20;
    case "working":
    case "thinking":
      return 24;
    case "listening":
      return level > 0.01 ? 60 : 24;
    default:
      return 0;
  }
}

const NO_GAZE: Vec = { x: 0, y: 0.3 };
/** Seconds without pointer movement after which glances stop. */
const DEEP_REST = 45;
/** Pointer movement (in gaze units) that counts as the person doing something. */
const MEANINGFUL = 0.05;
/** Reduced motion: length of the cross-fade between states. */
const FADE = 0.13;
/** Blink timing: close, hold, open (seconds), and the gap inside a pair. */
const BLINK_CLOSE = 0.065;
const BLINK_HOLD = 0.02;
const BLINK_OPEN = 0.11;
const BLINK_PAIR_GAP = 0.1;
/** Pill eyes: how long they settle after a blink. */
const REBOUND = 0.42;

/** The settle after a blink of a pill eye, `e` seconds after it opened. */
export function reboundCurve(e: number): number {
  if (e < 0 || e >= REBOUND) return 0;
  return Math.sin((2 * Math.PI * e) / 0.26) * Math.exp(-e / 0.1);
}

export class FaceSim {
  private input: SimInput;
  private pose: Pose;
  private vel: Pose;
  private onFrame: (frame: Frame) => void;

  private raf = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private last = 0;
  /** True while the loop sleeps until the next scheduled thing; the clock
   * restarts after such a pause instead of integrating across it. */
  private resting = true;
  private running = false;

  private state: FaceState;
  private stateAt: number;
  private lastGazeAt: number;
  private prevGaze: Vec | null = null;
  private lastTransitionBlink = -Infinity;

  private fix: Vec = { x: 0, y: 0.3 };
  private jitter: Vec = { x: 0, y: 0 };
  private lastSaccade = 0;
  private nextMicro: number;
  private nextWander: number;
  private wander: { to: Vec; until: number } | null = null;

  private blinkAt: number;
  private blinkStart = -1;
  private blinkClose = BLINK_CLOSE;
  private blinkLen = BLINK_CLOSE + BLINK_HOLD + BLINK_OPEN;
  /** Blinks left in the current burst, the scheduled one included. */
  private blinksLeft = 1;
  /** When the last blink of a pill eye opened (its rebound starts there). */
  private reboundAt = -Infinity;

  private emote: { kind: IdleEmote; start: number; side: 1 | -1 } | null = null;
  private nextEmote: number;
  private lastEmote: IdleEmote | null = null;
  private lastRareAt = -Infinity;

  private level = 0;
  private levelSlow = 0;

  private lastFrame: Frame | null = null;
  private fadeFrom: Frame | null = null;
  private fadeStart = 0;

  /** The gaze rig (pill eyes with a motion style), else null. */
  private rig: Rig | null = null;

  constructor(input: SimInput, onFrame: (frame: Frame) => void) {
    const now = faceClock();
    this.input = input;
    this.onFrame = onFrame;
    this.state = input.state;
    this.stateAt = now;
    this.lastGazeAt = now;
    this.prevGaze = input.gaze;
    this.blinkAt = now + rand(1.2, 3.5);
    this.nextMicro = now + rand(1, 3);
    this.nextWander = now + rand(6, 12);
    this.nextEmote = now + this.emoteGap(now);
    const g = input.gaze ?? NO_GAZE;
    this.fix = { ...g };
    this.pose = targetPose(this.targetInput(now));
    this.vel = Object.fromEntries(CHANNELS.map((c) => [c, 0])) as Pose;
    this.syncRig(now);
  }

  /** Makes or drops the rig when the style (or the eye shape) changes. */
  private syncRig(now: number): void {
    const style = styleOf(this.input);
    if (style === "classic") {
      this.rig = null;
      return;
    }
    if (this.rig?.style === style) {
      this.rig.setTuning?.(this.input.rigTuning);
      return;
    }
    this.rig = makeRig(style, now, { x: this.pose.gx, y: this.pose.gy }, this.input.rigTuning);
    this.rig?.noteCursor(this.input.gaze, now);
  }

  /** The first frame, before any animation. */
  frame(): Frame {
    const now = faceClock();
    return this.makeFrame(now, 0, 0);
  }

  update(next: SimInput): void {
    const prev = this.input;
    this.input = next;
    const now = faceClock();
    if (next.face.motion.blink !== prev.face.motion.blink) this.blinkAt = now + this.blinkGap();
    this.syncRig(now);
    const g = next.gaze;
    const p = this.prevGaze;
    if (g && (!p || Math.abs(g.x - p.x) > 0.004 || Math.abs(g.y - p.y) > 0.004)) {
      const wasAsleep = this.effectiveState(now) === "sleeping" && this.input.state === "idle";
      this.lastGazeAt = now;
      // The person moved the pointer for real: drop any daydreaming glance.
      if (!p || Math.hypot(g.x - p.x, g.y - p.y) > MEANINGFUL) {
        this.wander = null;
        this.jitter = { x: 0, y: 0 };
        this.nextWander = now + rand(6, 12);
      }
      this.prevGaze = g;
      this.rig?.noteCursor(g, now);
      if (wasAsleep) this.startBlinks(now + 0.25, 2);
    }
    this.wake();
  }

  /** Plays an idle emote now (editor preview, demo). */
  playEmote(kind: IdleEmote): void {
    const now = faceClock();
    this.emote = { kind, start: now, side: Math.random() < 0.5 ? 1 : -1 };
    this.wake();
  }

  start(): void {
    this.running = true;
    this.wake();
  }

  stop(): void {
    this.running = false;
    if (this.raf) cancelAnimationFrame(this.raf);
    if (this.timer) clearTimeout(this.timer);
    this.raf = 0;
    this.timer = null;
  }

  private wake(): void {
    if (!this.running || this.raf) return;
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (this.resting) this.last = faceClock();
    this.raf = requestAnimationFrame(this.tick);
  }

  private blinkGap(): number {
    const b = this.input.face.motion.blink;
    let gap = lerp(9, 2.3, b) * rand(0.55, 1.45);
    if (this.state === "working") gap *= 1.5;
    if (this.state === "thinking") gap *= 0.85;
    return clamp(gap, 0.9, 16);
  }

  private emoteGap(now: number): number {
    const deep = now - this.lastGazeAt > DEEP_REST ? 2 : 1;
    return (deep * rand(18, 45)) / (0.35 + this.input.face.motion.liveliness);
  }

  private pickEmote(now: number): IdleEmote | null {
    const list = this.input.face.idle_emotes.filter(
      (e) => !(RARE.includes(e) && now - this.lastRareAt < RARE_COOLDOWN) && !(e === this.lastEmote && this.input.face.idle_emotes.length > 1),
    );
    if (!list.length) return null;
    const total = list.reduce((s, e) => s + EMOTE_WEIGHT[e], 0);
    let x = Math.random() * total;
    for (const e of list) {
      x -= EMOTE_WEIGHT[e];
      if (x <= 0) return e;
    }
    return list[list.length - 1] ?? null;
  }

  private effectiveState(now: number): FaceState {
    const { state, autoSleep, face } = this.input;
    const mins = face.motion.sleepy_after_min;
    if (state === "idle" && autoSleep && mins > 0 && now - this.lastGazeAt > mins * 60) return "sleeping";
    return state;
  }

  private startBlinks(at: number, count: number): void {
    this.blinkAt = at;
    this.blinksLeft = count;
  }

  private blinkValue(now: number): number {
    if (this.blinkStart < 0) return 0;
    const e = now - this.blinkStart;
    if (e >= this.blinkLen) {
      if (this.input.face.eyes.shape === "pill") this.reboundAt = this.blinkStart + this.blinkLen;
      this.blinkStart = -1;
      return 0;
    }
    const c = this.blinkClose;
    const hold = BLINK_HOLD * (this.blinkLen / (BLINK_CLOSE + BLINK_HOLD + BLINK_OPEN));
    if (e < c) {
      const x = e / c;
      return x * x;
    }
    if (e < c + hold) return 1;
    const x = (e - c - hold) / (this.blinkLen - c - hold);
    return (1 - x) * (1 - x);
  }

  private targetInput(now: number): TargetInput {
    const state = this.state;
    const st = now - this.stateAt;
    const emote = this.emote && state === "idle" ? { kind: this.emote.kind, t: now - this.emote.start, side: this.emote.side } : null;
    let look: Vec = { x: this.fix.x + this.jitter.x, y: this.fix.y + this.jitter.y };
    if (this.wander && now < this.wander.until) look = this.wander.to;
    const g = this.input.gaze ?? NO_GAZE;
    const amp = state === "driving" ? 1 : 0.25 + 0.75 * this.input.face.motion.curiosity;
    // With the rig the head turn is drawn from the rig, not leaned by the pose.
    const lean = this.rig ? { x: 0, y: 0 } : { x: clamp(g.x, -1, 1) * amp, y: clamp(g.y, -1, 1) * amp };
    return {
      face: this.input.face,
      state,
      st,
      t: now,
      look,
      lean,
      level: this.level,
      emote,
      reduced: this.input.reduced,
      compact: this.input.compact,
    };
  }

  private updateGaze(now: number): void {
    const { face, reduced } = this.input;
    const state = this.state;
    const driving = state === "driving";
    const cur = driving ? 1 : face.motion.curiosity;
    const live = face.motion.liveliness;
    const g = this.input.gaze ?? NO_GAZE;
    const amp = driving ? 1 : 0.25 + 0.75 * cur;
    const want: Vec = { x: clamp(g.x, -1, 1) * amp, y: clamp(g.y, -1, 1) * amp };
    if (reduced || driving || this.rig) {
      // Reduced motion and driving: straight onto the target, no saccades.
      // With the rig the saccades happen there; only the daydreaming glances
      // stay here (and the head style has its own attention instead).
      if (this.rig && !reduced && !driving && this.rig.style !== "head" && this.rig.style !== "smooth") this.daydream(now, state, live);
      else this.wander = null;
      this.fix = want;
      this.jitter = { x: 0, y: 0 };
      return;
    }
    const thresh = 0.05 + (1 - cur) * 0.12;
    const gap = 0.06 + (1 - cur) * 0.22;
    const dist = Math.hypot(want.x - this.fix.x, want.y - this.fix.y);
    if (dist > thresh && now - this.lastSaccade > gap) {
      // Saccades undershoot a touch, then a small correction lands.
      const under = dist > 0.3 ? rand(0.88, 0.96) : 1;
      this.fix = { x: this.fix.x + (want.x - this.fix.x) * under, y: this.fix.y + (want.y - this.fix.y) * under };
      this.lastSaccade = now;
      this.jitter = { x: 0, y: 0 };
    }
    // Deep rest: the pointer has not moved for a while, so no glances; only
    // blinks (and the odd habit) keep the face alive.
    const awake = now - this.lastGazeAt < DEEP_REST;
    if (state === "idle" && live > 0 && awake && now > this.nextMicro) {
      const a = 0.025 + 0.06 * live;
      this.jitter = { x: rand(-a, a), y: rand(-a, a) * 0.7 };
      this.nextMicro = now + rand(2.2, 6.5) / (0.5 + live);
    }
    this.daydream(now, state, live);
  }

  /** A glance elsewhere now and then when the pointer has rested a while. */
  private daydream(now: number, state: FaceState, live: number): void {
    const awake = now - this.lastGazeAt < DEEP_REST;
    if (state === "idle" && live > 0 && awake && now - this.lastGazeAt > 5 && now > this.nextWander && !this.emote) {
      this.wander = { to: { x: rand(-0.75, 0.75), y: rand(-0.35, 0.45) }, until: now + rand(0.6, 1.4) };
      this.nextWander = now + rand(5, 14) / (0.3 + live);
    }
    if (this.wander && now >= this.wander.until) this.wander = null;
  }

  private tick = (ms: number): void => {
    this.raf = 0;
    if (!this.running) return;
    const now = faceClock(ms);
    const dt = clamp(now - this.last, 0, 0.1);
    this.last = now;
    this.resting = false;
    const { face, reduced, level } = this.input;

    // State changes start their clock. A blink marks a shift of attention,
    // never the onset of a question or a problem, and not too often.
    const eff = this.effectiveState(now);
    if (eff !== this.state) {
      const from = this.state;
      if (reduced && this.lastFrame) {
        this.fadeFrom = this.lastFrame;
        this.fadeStart = now;
      }
      this.state = eff;
      this.stateAt = now;
      this.emote = null;
      this.wander = null;
      this.jitter = { x: 0, y: 0 };
      if (!reduced) {
        if (from === "sleeping") this.startBlinks(now + 0.2, 2);
        else if (BLINK_INTO.has(eff) && now - this.lastTransitionBlink > BLINK_COOLDOWN) {
          this.startBlinks(now, 1);
          this.lastTransitionBlink = now;
        }
      }
      if (eff === "idle") this.nextEmote = now + this.emoteGap(now);
    }

    // Microphone level: 30 ms attack, 140 ms release.
    const lv = this.state === "listening" ? clamp(level, 0, 1) : 0;
    this.level += (lv - this.level) * (1 - Math.exp(-dt / (lv > this.level ? 0.03 : 0.14)));
    this.levelSlow += (this.level - this.levelSlow) * (1 - Math.exp(-dt / 0.22));

    this.updateGaze(now);

    // Emotes, only while idle and awake.
    if (this.emote && now - this.emote.start > EMOTE_LENGTH[this.emote.kind]) {
      if (this.emote.kind === "yawn") this.startBlinks(now + 0.1, 2);
      this.emote = null;
      this.nextEmote = now + this.emoteGap(now);
    }
    if (this.state === "idle" && !this.emote && !reduced && face.idle_emotes.length && now >= this.nextEmote) {
      const kind = this.pickEmote(now);
      if (kind) {
        this.lastEmote = kind;
        if (RARE.includes(kind)) this.lastRareAt = now;
        this.emote = { kind, start: now, side: Math.random() < 0.5 ? 1 : -1 };
      } else {
        this.nextEmote = now + this.emoteGap(now);
      }
    } else if (this.state !== "idle" && now >= this.nextEmote) {
      this.nextEmote = now + this.emoteGap(now);
    }

    // The state's wish for this moment; the rig turns its gaze into saccades
    // (and may ask for a blink timed with a large one).
    const target = targetPose(this.targetInput(now));
    const noBlinks = reduced || this.state === "sleeping" || this.state === "offline";
    let rigEye: Vec | null = null;
    if (this.rig) {
      const out = this.rig.step(now, dt, { want: { x: target.gx, y: target.gy }, cursor: this.input.gaze, state: this.state, reduced });
      rigEye = out.eye;
      if (out.blink && !noBlinks && this.blinkStart < 0) this.startBlinks(now, 1);
      if (!reduced) applyRig(target, this.rig.frame(now));
    }

    // Blinks: none while asleep, offline or in reduced motion.
    if (!noBlinks && this.blinkStart < 0 && now >= this.blinkAt) {
      this.blinkStart = now;
      const slow = face.motion.blink < 0.3 ? 1.25 : 1;
      const v = rand(0.92, 1.12) * slow * (this.rig?.blinkScale?.() ?? 1);
      this.blinkClose = BLINK_CLOSE * v;
      this.blinkLen = (BLINK_CLOSE + BLINK_HOLD + BLINK_OPEN) * v;
      this.blinksLeft -= 1;
      if (this.blinksLeft > 0) {
        this.blinkAt = now + this.blinkLen + BLINK_PAIR_GAP;
      } else {
        this.blinkAt = now + this.blinkGap();
        this.blinksLeft = Math.random() < 0.14 ? 2 : 1;
      }
    }
    if (noBlinks) {
      this.blinkStart = -1;
      if (this.blinkAt < now) this.blinkAt = now + 2;
    }
    const blink = this.blinkValue(now);

    // Springs, in substeps of at most 1/120 s.
    let moving = false;
    if (reduced) {
      this.pose = target;
    } else {
      const springs = new Map<Group, Spring>();
      const steps = Math.max(1, Math.ceil(dt / (1 / 120)));
      const h = dt / steps;
      for (const ch of CHANNELS) {
        // The rig owns the gaze (set below); its own busy() says when it moves.
        if (rigEye && (ch === "gx" || ch === "gy")) continue;
        const grp = GROUP[ch];
        let sp = springs.get(grp);
        if (!sp) {
          sp = springFor(grp, face, this.state);
          springs.set(grp, sp);
        }
        let x = this.pose[ch];
        let v = this.vel[ch];
        const to = target[ch];
        for (let i = 0; i < steps; i++) {
          v += (sp.k * (to - x) - sp.c * v) * h;
          x += v * h;
        }
        this.pose[ch] = x;
        this.vel[ch] = v;
        const eps = EPS[ch] ?? 0.004;
        if (Math.abs(to - x) > eps || Math.abs(v) > eps * 8) moving = true;
      }
    }
    if (rigEye) {
      // The rig owns the gaze: its saccades are drawn as they are.
      this.pose.gx = rigEye.x;
      this.pose.gy = rigEye.y;
      this.vel.gx = 0;
      this.vel.gy = 0;
      if (this.rig!.busy(now)) moving = true;
    }

    const st = now - this.stateAt;
    const timeline = TIMELINE[this.state];
    const inTimeline = !reduced && timeline !== undefined && st < timeline + 0.6;
    const fading = this.fadeFrom !== null && now - this.fadeStart < FADE;
    if (!fading) this.fadeFrom = null;
    const rigFps = this.rig && this.state !== "offline" ? this.rig.ambientFps(reduced) : 0;
    const fps = reduced ? (this.state === "listening" && this.level > 0.01 ? 30 : 0) : Math.max(ambientFps(this.state, this.level), rigFps);
    // Fast things (a blink, an emote, a one-shot timeline, a cross-fade) get
    // every frame; the slow ambient motion of a state gets its capped rate.
    // The symbols beside the eyes fade in over 150 ms after a state change.
    const symbolIn = !reduced && SYMBOL_STATES.has(this.state) && st < 0.2;
    const settling = !reduced && now - this.reboundAt < REBOUND;
    const rigBusy = !reduced && this.rig !== null && this.rig.busy(now);
    const fast = blink > 0 || this.blinkStart >= 0 || this.emote !== null || inTimeline || fading || symbolIn || settling || rigBusy;
    const capped = fps > 0 && !fast;
    const busy = fast || (moving && fps === 0);

    const frame = this.makeFrame(now, blink, st);
    if (fading && this.fadeFrom) frame.fade = { from: this.fadeFrom, a: smooth((now - this.fadeStart) / FADE) };
    this.lastFrame = frame;
    this.onFrame(frame);

    if (busy) {
      this.raf = requestAnimationFrame(this.tick);
      return;
    }
    if (capped) {
      this.timer = setTimeout(() => {
        this.timer = null;
        this.wake();
      }, 1000 / fps);
      return;
    }
    // At rest: sleep until the next scheduled thing.
    const nexts = noBlinks ? [] : [this.blinkAt];
    if (!reduced && this.state === "idle") {
      if (face.idle_emotes.length) nexts.push(this.nextEmote);
      if (face.motion.liveliness > 0 && now - this.lastGazeAt < DEEP_REST) {
        nexts.push(this.nextMicro);
        if (now - this.lastGazeAt > 5) nexts.push(this.nextWander);
        else nexts.push(this.lastGazeAt + 5.01);
      }
    }
    if (this.rig && !reduced) nexts.push(...this.rig.wakeTimes());
    if (this.input.state === "idle" && this.input.autoSleep && face.motion.sleepy_after_min > 0)
      nexts.push(this.lastGazeAt + face.motion.sleepy_after_min * 60 + 0.01);
    // Only future moments count: a stale one must never turn into polling.
    const future = nexts.filter((x) => x > now + 0.001);
    this.resting = true;
    if (!future.length) return;
    // Face seconds to real milliseconds (the lab can slow the clock down).
    const wait = Math.max(0.016, Math.min(...future) - now);
    this.timer = setTimeout(() => {
      this.timer = null;
      this.wake();
    }, (wait * 1000) / faceTimeScale());
  };

  private makeFrame(now: number, blink: number, st: number): Frame {
    const rebound = this.input.reduced ? 0 : reboundCurve(now - this.reboundAt) * (this.rig?.reboundGain?.() ?? 1);
    return {
      ...(rebound !== 0 ? { rebound } : {}),
      ...(this.rig ? { rig: this.rig.frame(now) } : {}),
      pose: { ...this.pose },
      blinkL: blink,
      blinkR: blink,
      level: this.level,
      levelSlow: this.levelSlow,
      state: this.state,
      st,
      t: now,
    };
  }
}

/** One settled frame for a still picture (thumbnails, screenshots). */
export function stillFrame(
  face: FaceConfig,
  state: FaceState,
  gaze: Vec | null,
  level: number,
  compact: boolean,
  emote?: EmoteRun | null,
  blink = 0,
): Frame {
  const st = HERO_TIME[state] ?? 1.2;
  const g = gaze ?? NO_GAZE;
  const amp = state === "driving" ? 1 : 0.25 + 0.75 * face.motion.curiosity;
  const look = { x: clamp(g.x, -1, 1) * amp, y: clamp(g.y, -1, 1) * amp };
  const pose = targetPose({ face, state, st, t: st, look, lean: look, level, emote: emote ?? null, reduced: false, compact });
  return { pose, blinkL: blink, blinkR: blink, level, levelSlow: level * 0.8, state, st, t: st };
}
