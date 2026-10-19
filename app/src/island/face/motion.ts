// How the face moves. A pose is a set of named numbers (where the eyes look,
// how far each lid is closed, brows, mouth, head). Each state and emote
// says where the pose should go (`targetPose`); springs carry the pose
// there. Blinks are a fast curve laid on top of the lids after the springs,
// so they never wobble.
//
// Energy: FaceSim runs requestAnimationFrame only while something moves.
// When every spring has settled and no timeline is running it stops and
// sets one timer for the next scheduled thing (blink, glance, emote,
// falling asleep). States that keep moving on their own (sleeping,
// working, thinking, approval, listening) run at a capped frame rate.

import type { FaceConfig, FaceState, IdleEmote } from "./schema";

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
  "dim", // offline drain 0..1
  "tongue", // tongue out 0..1
  "glow", // added glow
  "ear", // accessory twitch (degrees)
  "gl", // glasses lift: + pushed up, - slid down the nose (in lens heights)
  "glr", // glasses tilt, degrees
  "ax", // trailing copy of the head for antenna and halo (lags behind)
  "ay",
] as const;

export type Channel = (typeof CHANNELS)[number];
export type Pose = Record<Channel, number>;

type Group = "gaze" | "head" | "face" | "slow" | "pop" | "wobble";
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
  tongue: "face",
  glow: "slow",
  ear: "wobble",
  gl: "pop",
  glr: "face",
  ax: "wobble",
  ay: "wobble",
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
  /** Smoothed microphone level 0..1, with a slower copy for the outer wave. */
  level: number;
  levelSlow: number;
  /** The state being shown (idle may become sleeping by itself). */
  state: FaceState;
  /** Seconds since that state began, and a running clock. */
  st: number;
  t: number;
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

/** How long the one-shot states play before they hold. */
const TIMELINE: Partial<Record<FaceState, number>> = { accepted: 0.8, done: 1.5, error: 2.6 };

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
}

/** The pose a state wants at a moment. Pure. */
export function targetPose(input: TargetInput): Pose {
  const { face, state, st, t, look, lean, level, emote, reduced } = input;
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
    tongue: 0,
    glow: 0,
    ear: 0,
    gl: 0,
    glr: 0,
    ax: 0,
    ay: 0,
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
      p.dim = 0.2;
      p.gl = -0.26;
      p.glr = 4;
      break;
    }

    case "listening": {
      p.gx = look.x * 0.25;
      p.gy = 0.1 + look.y * 0.12;
      both("u", rest * 0.25);
      both("l", 0.02);
      both("s", 1.04 + level * 0.07);
      p.pupil = 1.12 + level * 0.18;
      brows(0.22 + level * 0.4, 4);
      p.smile = Math.max(baseSmile, 0.2);
      p.open = baseOpen * 0.5;
      p.hr = 6;
      p.hx = 0;
      p.hy = -0.5;
      p.glow = level * 0.35;
      break;
    }

    case "accepted": {
      const nod = envelope(st, 0.04, 0.5, 0.12, 0.3);
      const squash = st < 0.07 ? 1 - st / 0.07 : 0;
      both("u", 0);
      both("l", 0.3);
      p.lowCurve = 0.5;
      both("s", 1.05 - squash * 0.1);
      brows(0.4, 2);
      p.smile = Math.max(0.85, baseSmile);
      p.open = baseOpen;
      p.blush = 0.2;
      p.hy = nod * 4.5;
      p.hr = 3;
      p.hx = 0;
      p.gx = look.x * 0.4;
      p.gy = 0.15;
      p.ear = st < 0.3 ? 10 : 0;
      break;
    }

    case "thinking": {
      const spots: Vec[] = [
        { x: 0.6, y: -0.7 },
        { x: -0.35, y: -0.75 },
        { x: 0.25, y: -0.85 },
        { x: 0.7, y: -0.45 },
        { x: -0.15, y: -0.6 },
      ];
      const i = Math.floor(st / 1.05) % spots.length;
      const spot = spots[i] ?? spots[0]!;
      p.gx = spot.x;
      p.gy = spot.y;
      p.uL = 0;
      p.uR = rest * 0.5 + 0.12;
      p.lR = 0.2;
      p.lL = 0.04;
      p.bLl = 0.6;
      p.bLa = 4;
      p.bRl = -0.05;
      p.bRa = -7;
      p.smile = baseSmile * 0.3 - 0.08;
      p.open = 0;
      p.mouthX = 0.4;
      p.hr = -6;
      p.hx = 2;
      p.hy = -1.5;
      p.glr = -3;
      break;
    }

    case "working": {
      // Reading: the eyes hop along a line, then jump back for the next.
      const period = 1.9;
      const ph = (st % period) / period;
      const line = Math.floor(st / period) % 3;
      p.gx = ph < 0.84 ? -0.5 + Math.floor(ph * 6) * 0.2 : -0.5;
      p.gy = 0.42 + line * 0.07;
      both("u", rest * 0.6 + 0.2);
      both("l", 0.14);
      p.lowCurve = -0.1;
      both("t", 0.22);
      brows(-0.18, -9);
      p.smile = baseSmile * 0.3;
      p.open = 0;
      p.hr = Math.sin(st * 1.15) * 2.2;
      p.hx = Math.sin(st * 0.6) * 1.2;
      p.hy = 1.6 + Math.sin(st * 2.3) * 0.45;
      break;
    }

    case "driving": {
      p.gx = look.x;
      p.gy = look.y;
      p.hx = lean.x * 4.5;
      p.hy = lean.y * 2;
      p.hr = lean.x * 4;
      both("u", rest * 0.3);
      both("s", 1.03);
      brows(0.08, -8);
      p.smile = Math.max(0.25, baseSmile * 0.6);
      p.open = 0;
      p.mouthX = 0.3;
      p.tongue = 1;
      break;
    }

    case "approval": {
      const beat = Math.sin(st * Math.PI * 2 * 0.85);
      both("u", 0);
      both("l", 0.02);
      both("s", 1.12 + beat * 0.025);
      p.pupil = 1.42;
      brows(0.55, 15);
      p.smile = 0.08;
      p.open = Math.max(0.1, baseOpen * 0.4);
      p.hr = 8;
      p.hx = lean.x * 2;
      p.hy = -1;
      p.glow = 0.3 + beat * 0.15;
      p.blush = 0.1;
      p.gl = 0.62;
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
      p.blush = 0.35;
      p.hy = -5 * hop;
      p.hr = 5 * Math.sin(st * 8) * Math.exp(-st * 2.2) + 3;
      p.hx = 0;
      p.gx = 0;
      p.gy = 0;
      p.ear = st < 0.4 ? -12 : 0;
      p.gl = 0.18 * hop;
      break;
    }

    case "error": {
      const dart = st < 0.95;
      p.gx = dart ? (Math.sin(st * 13) > 0 ? 0.55 : -0.55) : 0.3;
      p.gy = dart ? 0.05 : 0.38;
      p.sL = 1.1;
      p.sR = 0.86;
      p.uL = 0;
      p.uR = rest * 0.5 + 0.26;
      p.lR = 0.1;
      p.pupil = 0.62;
      p.bLl = 0.6;
      p.bLa = 7;
      p.bRl = -0.15;
      p.bRa = -12;
      p.smile = -0.45;
      p.open = 0.05;
      p.mouthX = -0.3;
      p.hr = 8 + 6 * Math.sin(st * 17) * Math.exp(-st * 3.5);
      p.hx = 0;
      p.hy = 0.5;
      p.glr = 9;
      p.gl = -0.12;
      break;
    }

    case "offline": {
      p.gx = 0;
      p.gy = 0.4;
      both("u", Math.max(0.56, rest));
      both("l", 0.1);
      p.pupil = 0.9;
      brows(-0.25, 4);
      p.smile = baseSmile * 0.2 - 0.05;
      p.open = 0;
      p.hx = 0;
      p.hr = 0;
      p.hy = 2.5;
      p.dim = 1;
      p.gl = -0.3;
      break;
    }
  }

  if (state === "idle" && emote) applyEmote(p, emote, rest, baseSmile);

  // Eyelids follow the eyes a little: looking down lowers them.
  const follow = p.gy > 0 ? p.gy * 0.1 : p.gy * 0.1;
  p.uL = clamp(p.uL + follow, 0, 1);
  p.uR = clamp(p.uR + follow, 0, 1);

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
      p.gl -= 0.2 * k;
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
      p.gl -= 0.22 * k;
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
      p.ear = -14 * k;
      p.gl += 0.4 * k;
      break;
    }
  }
}

/** The moment of each state a still picture shows. */
export const HERO_TIME: Partial<Record<FaceState, number>> = { accepted: 0.3, done: 0.55, error: 1.4, sleeping: 2.2 };

// ---------- the running face ----------

export interface SimInput {
  face: FaceConfig;
  state: FaceState;
  gaze: Vec | null;
  level: number;
  reduced: boolean;
  autoSleep: boolean;
}

interface Spring {
  k: number;
  c: number;
}

function springFor(g: Group, face: FaceConfig): Spring {
  const cur = face.motion.curiosity;
  const live = face.motion.liveliness;
  const make = (k: number, zeta: number) => ({ k, c: 2 * zeta * Math.sqrt(k) });
  switch (g) {
    case "gaze":
      return make(380 + 620 * cur, 0.82);
    case "head":
      return make(70 + 70 * live, 0.5 + 0.2 * (1 - live));
    case "face":
      return make(230, 0.78);
    case "slow":
      return make(30, 1);
    case "pop":
      return make(300, 0.48);
    case "wobble":
      return make(45, 0.22);
  }
}

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
  glow: 0.01,
};

const rand = (lo: number, hi: number) => lo + Math.random() * (hi - lo);

/** Seconds of motion per state that still needs frames, and its fps cap. */
function ambientFps(state: FaceState, level: number): number {
  switch (state) {
    case "sleeping":
      return 20;
    case "working":
    case "thinking":
      return 24;
    case "approval":
      return 30;
    case "listening":
      return level > 0.01 ? 60 : 30;
    default:
      return 0;
  }
}

const NO_GAZE: Vec = { x: 0, y: 0.3 };
/** Seconds without pointer movement after which glances stop. */
const DEEP_REST = 45;

export class FaceSim {
  private input: SimInput;
  private pose: Pose;
  private vel: Pose;
  private springs = new Map<Group, Spring>();
  private onFrame: (frame: Frame) => void;

  private raf = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private last = 0;
  private running = false;

  private state: FaceState;
  private stateAt: number;
  private lastGazeAt: number;
  private prevGaze: Vec | null = null;

  private fix: Vec = { x: 0, y: 0.3 };
  private jitter: Vec = { x: 0, y: 0 };
  private lastSaccade = 0;
  private nextMicro: number;
  private nextWander: number;
  private wander: { to: Vec; until: number } | null = null;

  private blinkAt: number;
  private blinkStart = -1;
  private blinkLen = 0.22;
  private blinkClose = 0.075;
  private doubleBlink = false;

  private emote: { kind: IdleEmote; start: number; side: 1 | -1 } | null = null;
  private nextEmote: number;
  private lastEmote: IdleEmote | null = null;

  private level = 0;
  private levelSlow = 0;

  constructor(input: SimInput, onFrame: (frame: Frame) => void) {
    const now = performance.now() / 1000;
    this.input = input;
    this.onFrame = onFrame;
    this.state = input.state;
    this.stateAt = now;
    this.lastGazeAt = now;
    this.prevGaze = input.gaze;
    this.blinkAt = now + rand(1.2, 3.5);
    this.nextMicro = now + rand(1, 3);
    this.nextWander = now + rand(6, 12);
    this.nextEmote = now + this.emoteGap();
    this.rebuildSprings();
    const g = input.gaze ?? NO_GAZE;
    this.fix = { ...g };
    this.pose = targetPose(this.targetInput(now));
    this.vel = Object.fromEntries(CHANNELS.map((c) => [c, 0])) as Pose;
  }

  /** The first frame, before any animation. */
  frame(): Frame {
    const now = performance.now() / 1000;
    return this.makeFrame(now, 0, 0);
  }

  update(next: SimInput): void {
    const prev = this.input;
    this.input = next;
    const now = performance.now() / 1000;
    if (next.face !== prev.face) {
      this.rebuildSprings();
      if (next.face.motion.blink !== prev.face.motion.blink) this.blinkAt = now + this.blinkGap();
    }
    const g = next.gaze;
    const p = this.prevGaze;
    if (g && (!p || Math.abs(g.x - p.x) > 0.004 || Math.abs(g.y - p.y) > 0.004)) {
      const wasAsleep = this.effectiveState(now) === "sleeping" && this.input.state === "idle";
      this.lastGazeAt = now;
      this.prevGaze = g;
      if (wasAsleep) this.wakeUp(now);
    }
    this.wake();
  }

  /** Plays an idle emote now (editor preview, demo). */
  playEmote(kind: IdleEmote): void {
    const now = performance.now() / 1000;
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
    this.last = performance.now() / 1000;
    this.raf = requestAnimationFrame(this.tick);
  }

  private rebuildSprings(): void {
    for (const g of ["gaze", "head", "face", "slow", "pop", "wobble"] as Group[]) this.springs.set(g, springFor(g, this.input.face));
  }

  private blinkGap(): number {
    const b = this.input.face.motion.blink;
    let gap = lerp(9, 2.3, b) * rand(0.55, 1.45);
    const s = this.state;
    if (s === "working") gap *= 1.5;
    if (s === "offline") gap *= 2.4;
    if (s === "thinking") gap *= 0.85;
    return clamp(gap, 0.9, 16);
  }

  private emoteGap(): number {
    const now = performance.now() / 1000;
    const rest = now - this.lastGazeAt > DEEP_REST ? 2 : 1;
    return (rest * rand(14, 38)) / (0.35 + this.input.face.motion.liveliness);
  }

  private effectiveState(now: number): FaceState {
    const { state, autoSleep, face } = this.input;
    const mins = face.motion.sleepy_after_min;
    if (state === "idle" && autoSleep && mins > 0 && now - this.lastGazeAt > mins * 60) return "sleeping";
    return state;
  }

  private wakeUp(now: number): void {
    this.startBlink(now + 0.25, true);
  }

  private startBlink(at: number, double = false): void {
    this.blinkAt = at;
    this.doubleBlink = double;
  }

  private blinkValue(now: number): number {
    if (this.blinkStart < 0) return 0;
    const e = now - this.blinkStart;
    if (e >= this.blinkLen) {
      this.blinkStart = -1;
      return 0;
    }
    const c = this.blinkClose;
    const hold = 0.03;
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
    return {
      face: this.input.face,
      state,
      st,
      t: now,
      look,
      lean: { x: clamp(g.x, -1, 1) * amp, y: clamp(g.y, -1, 1) * amp },
      level: this.level,
      emote,
      reduced: this.input.reduced,
    };
  }

  private updateGaze(now: number): void {
    const { face, reduced } = this.input;
    const state = this.state;
    const cur = state === "driving" ? 1 : face.motion.curiosity;
    const live = face.motion.liveliness;
    const g = this.input.gaze ?? NO_GAZE;
    const amp = state === "driving" ? 1 : 0.25 + 0.75 * cur;
    const want: Vec = { x: clamp(g.x, -1, 1) * amp, y: clamp(g.y, -1, 1) * amp };
    if (reduced) {
      this.fix = want;
      this.jitter = { x: 0, y: 0 };
      this.wander = null;
      return;
    }
    const thresh = state === "driving" ? 0.03 : 0.05 + (1 - cur) * 0.12;
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
    if (state === "idle" && live > 0 && awake && now - this.lastGazeAt > 5 && now > this.nextWander && !this.emote) {
      this.wander = { to: { x: rand(-0.75, 0.75), y: rand(-0.35, 0.45) }, until: now + rand(0.6, 1.4) };
      this.nextWander = now + rand(5, 14) / (0.3 + live);
    }
    if (this.wander && now >= this.wander.until) this.wander = null;
  }

  private tick = (ms: number): void => {
    this.raf = 0;
    if (!this.running) return;
    const now = ms / 1000;
    const dt = clamp(now - this.last, 0, 1 / 20);
    this.last = now;
    const { face, reduced, level } = this.input;

    // State changes start their clock and hide the switch behind a blink.
    const eff = this.effectiveState(now);
    if (eff !== this.state) {
      const from = this.state;
      this.state = eff;
      this.stateAt = now;
      this.emote = null;
      if (eff !== "sleeping" && from !== "sleeping") this.startBlink(now);
      else if (from === "sleeping") this.startBlink(now + 0.2, true);
      if (eff === "idle") this.nextEmote = now + this.emoteGap();
    }

    // Microphone level: quick to rise, slower to fall.
    const lv = this.state === "listening" ? clamp(level, 0, 1) : 0;
    this.level += (lv - this.level) * (1 - Math.exp(-dt / (lv > this.level ? 0.03 : 0.14)));
    this.levelSlow += (this.level - this.levelSlow) * (1 - Math.exp(-dt / 0.22));

    this.updateGaze(now);

    // Emotes, only while idle and awake.
    if (this.emote && now - this.emote.start > EMOTE_LENGTH[this.emote.kind]) {
      if (this.emote.kind === "yawn") this.startBlink(now + 0.1, true);
      this.emote = null;
      this.nextEmote = now + this.emoteGap();
    }
    if (this.state === "idle" && !this.emote && !reduced && face.idle_emotes.length && now >= this.nextEmote) {
      const pool = face.idle_emotes.length > 1 ? face.idle_emotes.filter((e) => e !== this.lastEmote) : face.idle_emotes;
      const kind = pool[Math.floor(Math.random() * pool.length)] ?? face.idle_emotes[0]!;
      this.lastEmote = kind;
      this.emote = { kind, start: now, side: Math.random() < 0.5 ? 1 : -1 };
    } else if (this.state !== "idle" && now >= this.nextEmote) {
      this.nextEmote = now + this.emoteGap();
    }

    // Blinks.
    if (this.blinkStart < 0 && now >= this.blinkAt && this.state !== "sleeping") {
      this.blinkStart = now;
      const slow = (this.state === "offline" ? 1.6 : 1) * (face.motion.blink < 0.3 ? 1.3 : 1);
      this.blinkClose = 0.07 * slow * rand(0.9, 1.15);
      this.blinkLen = (0.07 + 0.03 + 0.13) * slow * rand(0.9, 1.2);
      if (this.doubleBlink) {
        this.doubleBlink = false;
        this.blinkAt = now + this.blinkLen + 0.09;
      } else {
        this.blinkAt = now + this.blinkGap();
        if (Math.random() < 0.14) this.doubleBlink = true;
        if (this.doubleBlink) this.blinkAt = now + this.blinkLen + 0.09;
      }
    }
    if (this.state === "sleeping" && this.blinkAt < now) this.blinkAt = now + 2;
    const blink = this.blinkValue(now);

    // Springs.
    const target = targetPose(this.targetInput(now));
    let moving = false;
    if (reduced) {
      this.pose = target;
    } else {
      const steps = Math.max(1, Math.ceil(dt / (1 / 120)));
      const h = dt / steps;
      for (const ch of CHANNELS) {
        const sp = this.springs.get(GROUP[ch])!;
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

    const st = now - this.stateAt;
    const timeline = TIMELINE[this.state];
    const inTimeline = timeline !== undefined && st < timeline + 0.6;
    const fps = reduced ? (this.state === "listening" && this.level > 0.01 ? 30 : 0) : ambientFps(this.state, this.level);
    // Fast things (a blink, an emote, a one-shot timeline) get every frame;
    // the slow ambient motion of a state gets its capped rate.
    const fast = blink > 0 || this.blinkStart >= 0 || this.emote !== null || inTimeline;
    const capped = fps > 0 && !fast;
    const busy = fast || (moving && fps === 0);

    this.onFrame(this.makeFrame(now, blink, st));

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
    const nexts = [this.blinkAt];
    if (!reduced && this.state === "idle") {
      if (face.idle_emotes.length) nexts.push(this.nextEmote);
      if (face.motion.liveliness > 0 && now - this.lastGazeAt < DEEP_REST) {
        nexts.push(this.nextMicro);
        if (now - this.lastGazeAt > 5) nexts.push(this.nextWander);
        else nexts.push(this.lastGazeAt + 5.01);
      }
    }
    if (this.input.state === "idle" && this.input.autoSleep && face.motion.sleepy_after_min > 0)
      nexts.push(this.lastGazeAt + face.motion.sleepy_after_min * 60 + 0.01);
    // Only future moments count: a stale one must never turn into polling.
    const future = nexts.filter((x) => x > now + 0.001);
    const wait = future.length ? Math.max(0.016, Math.min(...future) - now) : 1;
    this.timer = setTimeout(() => {
      this.timer = null;
      this.wake();
    }, wait * 1000);
  };

  private makeFrame(now: number, blink: number, st: number): Frame {
    return {
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
export function stillFrame(face: FaceConfig, state: FaceState, gaze: Vec | null, level: number, emote?: EmoteRun | null): Frame {
  const st = HERO_TIME[state] ?? 1.2;
  const g = gaze ?? NO_GAZE;
  const amp = state === "driving" ? 1 : 0.25 + 0.75 * face.motion.curiosity;
  const look = { x: clamp(g.x, -1, 1) * amp, y: clamp(g.y, -1, 1) * amp };
  const pose = targetPose({ face, state, st, t: st, look, lean: look, level, emote: emote ?? null, reduced: false });
  return { pose, blinkL: 0, blinkR: 0, level, levelSlow: level * 0.8, state, st, t: st };
}
