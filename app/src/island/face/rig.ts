// The gaze rig for pill eyes: how eyes without pupils show where they look.
// Pill eyes have nothing inside to move, so sliding the whole eye after the
// pointer reads as blobs crawling. The rig replaces that with what Anki's
// Cozmo and Vector do with their rounded-rect eyes, in three steps that
// build on each other (the face lab compares them side by side):
//
//   cozmo  Perspective plus saccades. The gaze is sampled; once it has moved
//          far enough the eyes jump (60-120 ms ease-out, a small overshoot,
//          a settle) and then hold. Looking to a side turns the face: the
//          eye on that side grows taller, the far one gets smaller and
//          narrower, the gap closes toward the gaze side. Large shifts may
//          come with a blink, and tiny micro-saccades keep a fixation alive.
//   gloss  The same motion, and each bean is drawn with volume: a gradient,
//          a darker rim on the far side and a specular highlight lit from a
//          fixed light at the top left, so it slides across the eye against
//          the turn (Face.tsx draws it; the rig only says how far).
//   head   The same, plus a second layer: the eyes lead, the head (the whole
//          face plane, eyes and mouth) follows on a slower spring and turns
//          as if the features sat on a sphere. On top of it an attention
//          model: it tracks a moving pointer, gets bored of a resting one and
//          glances away, startles at a flick, looks down with a happy squint
//          when the pointer comes close, and breathes.
//
// "classic" is the shipped behaviour: no rig at all (motion.ts keeps its own
// gaze springs and Face.tsx its sliding eyes).
//
// Everything here is plain math on numbers, driven by FaceSim's clock, so it
// runs in node tests and inside the one animation loop FaceSim already owns.

import type { FaceState } from "./schema";

export interface Vec {
  x: number;
  y: number;
}

export type MotionStyle = "classic" | "cozmo" | "gloss" | "head";
export const MOTION_STYLES: readonly MotionStyle[] = ["classic", "cozmo", "gloss", "head"];

/** Saccades: thresholds in gaze units (the -1..1 vector Face gets), times in seconds. */
export const SACCADE = {
  /** A target this far from the fixation triggers a jump at once. */
  big: 0.2,
  /** A smaller offset jumps only after the target has rested for `dwell`. */
  small: 0.06,
  dwell: 0.09,
  /** Minimum time between two jumps (plus up to `refractJitter`). */
  refract: 0.16,
  refractJitter: 0.06,
  /** Jump length: min at amplitude 0, max at amplitude `durFullAt` and up. */
  durMin: 0.06,
  durMax: 0.12,
  durFullAt: 1.2,
  /** Overshoot as a fraction of the jump, growing with the amplitude. */
  overMin: 0.05,
  overMax: 0.11,
  /** Settle back from the overshoot. */
  settle: 0.1,
  /** Jumps at least this long may come with a blink (chance `blinkChance`);
   * a reversal of direction counts from `blinkReverseAmp`. */
  blinkAmp: 0.85,
  blinkReverseAmp: 0.5,
  blinkChance: 0.5,
  blinkCooldown: 1.4,
};

/** Micro-saccades while fixating: tiny jumps around the fixation. */
export const MICRO = {
  quietFor: 0.35,
  gapMin: 0.8,
  gapMax: 2.4,
  ampMin: 0.07,
  ampMax: 0.13,
  dur: 0.04,
};

/** The head layer (style "head"): a spring after the eyes' fixation. */
export const HEAD = {
  /** How much of the eyes' fixation the head turns to; the rest stays as
   * the eyes looking out of the corner of the head. */
  follow: 0.72,
  k: 38,
  zeta: 0.52,
  /** Breath: period (s), scale amplitude, drift (face units). */
  breathPeriod: 4.6,
  breathScale: 0.011,
  breathDrift: 0.35,
  /** Frame rate while only breathing (energy: the island is always on). */
  breathFps: 10,
};

/** The attention model (style "head", idle only). */
export const ATTENTION = {
  /** Pointer movement (gaze units) that counts as the person doing something. */
  meaningful: 0.03,
  /** Resting pointer: bored after this long, then glances away and back. */
  boredAfter: 3.2,
  awayMin: 1.0,
  awayMax: 2.4,
  backMin: 1.6,
  backMax: 3.6,
  /** Long rest: glances get longer, the face drifts toward straight ahead. */
  deepAfter: 20,
  /** A pointer this close (gaze length) is "at the face": look down at it, happy squint. */
  near: 0.22,
  nearLookDown: 0.4,
  /** Flick: pointer speed (gaze units per second, smoothed) for a startle. */
  flick: 4.5,
  flickTau: 0.05,
  surpriseLen: 0.7,
  surpriseCooldown: 2.5,
  squintTau: 0.22,
};

/** Cozmo perspective (in eye half widths hx / half heights hy / spacing d). */
export const COZMO = {
  /** The pair moves toward the gaze side by this many hx at full gaze. */
  shiftX: 0.65,
  /** And down / up by this many hy. */
  shiftY: 0.34,
  /** The gap between the eyes closes by this fraction at full gaze. */
  squeeze: 0.2,
  /** Near eye: taller / wider; far eye: shorter / narrower (fractions at full gaze). */
  nearH: 0.22,
  nearW: 0.06,
  farH: 0.18,
  farW: 0.3,
  /** Looking up stretches the eyes a little, looking down squashes them. */
  upH: 0.08,
  upW: -0.04,
  downH: -0.17,
  downW: 0.05,
  /** The mouth follows the pair and foreshortens. */
  mouthFollow: 0.9,
  mouthSquashX: 0.15,
  mouthSquashY: 0.1,
};

/** Sphere for the head layer: radius in eye spacings, max turn (degrees),
 * camera distance in radii. */
export const SPHERE = {
  radius: 2.1,
  yaw: 16,
  pitch: 11,
  camera: 3.2,
  /** How much Cozmo perspective rides on top of the turned head: from the
   * eyes' offset inside the head, and from the head turn itself. */
  relGain: 1.4,
  headGain: 0.45,
};

/** Gloss: where the highlight sits and how far it slides with the turn. */
export const GLOSS = {
  /** Highlight centre in bean fractions (x from the centre, + = outer side; y from the centre, + down). */
  x: -0.2,
  y: -0.27,
  /** Slides against the turn by this fraction of the width at full turn. */
  slide: 0.26,
  slideY: 0.14,
};

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const smooth = (t: number) => {
  const x = clamp(t, 0, 1);
  return x * x * (3 - 2 * x);
};
const len = (v: Vec) => Math.hypot(v.x, v.y);

/** The saccade's progress curve: an ease-out (cubic) that lands `over` past
 * the target at x = 1, then a settle back over `settleX` (in the same units
 * as x, a fraction of the jump length). Pure, for tests. */
export function saccadeCurve(x: number, over: number, settleX: number): number {
  if (x <= 0) return 0;
  if (x < 1) return (1 + over) * (1 - (1 - x) ** 3);
  const s = (x - 1) / Math.max(1e-6, settleX);
  if (s < 1) return 1 + over * (1 - smooth(s));
  return 1;
}

export interface RigFrame {
  style: MotionStyle;
  /** Where the head points (head style), else equal to the eyes. */
  head: Vec;
  /** Breath -1..1 (head style). */
  breathe: number;
  /** Startle 0..1 and happy squint 0..1 (head style, also applied to the pose). */
  surprise: number;
  squint: number;
  /** What the attention model is doing (for the lab caption). */
  mode: "track" | "bored" | "near" | "deep" | "state";
  saccading: boolean;
}

export interface RigInput {
  /** Where the state wants the eyes (targetPose gx / gy). */
  want: Vec;
  /** The pointer as a gaze vector, or null. */
  cursor: Vec | null;
  state: FaceState;
  reduced: boolean;
}

export interface RigOut {
  eye: Vec;
  /** Ask FaceSim for a blink now (timed with a large saccade). */
  blink: boolean;
}

interface Jump {
  from: Vec;
  to: Vec;
  t0: number;
  dur: number;
  over: number;
  micro: boolean;
}

export class GazeRig {
  readonly style: MotionStyle;
  private rand: () => number;

  private eye: Vec;
  /** The fixation: where the last real saccade aimed (micro-saccades wobble around it). */
  private fix: Vec;
  private jump: Jump | null = null;
  private lastJump = -Infinity;
  private refract = SACCADE.refract;
  private pendingSince: number | null = null;
  private lastDir: Vec = { x: 0, y: 0 };
  private lastBlinkAsk = -Infinity;
  private nextMicro: number;

  private head: Vec;
  private headV: Vec = { x: 0, y: 0 };

  private cursorPrev: Vec | null = null;
  private cursorPrevAt = -Infinity;
  private speed = 0;
  private lastMeaningful: number;
  private meaningfulFrom: Vec | null = null;
  private surpriseAt = -Infinity;
  private squint = 0;
  private squintTo = 0;
  private bored: { away: boolean; until: number; spot: Vec } | null = null;
  private mode: RigFrame["mode"] = "track";
  private breathe = 0;
  private lastStep = -Infinity;

  constructor(style: MotionStyle, now: number, start: Vec, rand: () => number = Math.random) {
    this.style = style;
    this.rand = rand;
    this.eye = { ...start };
    this.fix = { ...start };
    this.head = style === "head" ? { x: start.x * HEAD.follow, y: start.y * HEAD.follow } : { ...start };
    this.nextMicro = now + this.r(MICRO.gapMin, MICRO.gapMax);
    this.lastMeaningful = now;
  }

  private r(lo: number, hi: number): number {
    return lo + this.rand() * (hi - lo);
  }

  /** Called whenever the pointer's gaze vector changes (FaceSim.update). */
  noteCursor(g: Vec | null, now: number): void {
    if (!g) {
      this.cursorPrev = null;
      return;
    }
    const prev = this.cursorPrev;
    const dt = now - this.cursorPrevAt;
    if (prev && dt > 0.004 && dt < 0.15) {
      const v = Math.hypot(g.x - prev.x, g.y - prev.y) / dt;
      const k = 1 - Math.exp(-dt / ATTENTION.flickTau);
      this.speed += (v - this.speed) * k;
    } else {
      this.speed = 0;
    }
    this.cursorPrev = { ...g };
    this.cursorPrevAt = now;
    const from = this.meaningfulFrom;
    if (!from || Math.hypot(g.x - from.x, g.y - from.y) > ATTENTION.meaningful) {
      this.lastMeaningful = now;
      this.meaningfulFrom = { ...g };
      this.bored = null;
    }
    if (this.style === "head" && this.speed > ATTENTION.flick && now - this.surpriseAt > ATTENTION.surpriseCooldown) {
      this.surpriseAt = now;
    }
  }

  /** Startle 0..1 at a moment: up in 60 ms, held, down over the rest. */
  surpriseLevel(now: number): number {
    const e = now - this.surpriseAt;
    if (e < 0 || e > ATTENTION.surpriseLen) return 0;
    return Math.min(smooth(e / 0.06), smooth((ATTENTION.surpriseLen - e) / 0.4));
  }

  /** Where attention puts the eyes (head style, idle); else the state's wish. */
  private attend(now: number, inp: RigInput): Vec {
    if (this.style !== "head" || inp.reduced || inp.state !== "idle") {
      this.mode = inp.state === "idle" ? "track" : "state";
      this.bored = null;
      return inp.want;
    }
    const c = inp.cursor;
    if (c && len(c) < ATTENTION.near) {
      this.mode = "near";
      this.bored = null;
      return { x: inp.want.x, y: clamp(inp.want.y + ATTENTION.nearLookDown, -1, 1) };
    }
    const rest = now - this.lastMeaningful;
    if (rest < ATTENTION.boredAfter) {
      this.mode = "track";
      this.bored = null;
      return inp.want;
    }
    const deep = rest > ATTENTION.deepAfter;
    if (!this.bored || now >= this.bored.until) {
      const away = !this.bored || !this.bored.away;
      if (away) {
        // Somewhere else: away from the pointer, mostly level, sometimes
        // straight ahead when it has been quiet for long.
        let spot: Vec;
        if (deep && this.rand() < 0.4) spot = { x: this.r(-0.1, 0.1), y: this.r(-0.05, 0.12) };
        else {
          const side = inp.want.x > 0.15 ? -1 : inp.want.x < -0.15 ? 1 : this.rand() < 0.5 ? -1 : 1;
          spot = { x: side * this.r(0.35, 0.85), y: this.r(-0.45, 0.25) };
        }
        const k = deep ? 2 : 1;
        this.bored = { away: true, until: now + this.r(ATTENTION.awayMin, ATTENTION.awayMax) * k, spot };
      } else {
        const k = deep ? 0.7 : 1;
        this.bored = { away: false, until: now + this.r(ATTENTION.backMin, ATTENTION.backMax) * k, spot: inp.want };
      }
    }
    this.mode = deep ? "deep" : "bored";
    return this.bored.away ? this.bored.spot : inp.want;
  }

  step(now: number, dt: number, inp: RigInput): RigOut {
    this.lastStep = now;
    let blink = false;
    const reduced = inp.reduced;
    const want = this.attend(now, inp);

    if (reduced) {
      this.eye = { ...want };
      this.fix = { ...want };
      this.jump = null;
      this.head = this.style === "head" ? { x: want.x * HEAD.follow, y: want.y * HEAD.follow } : { ...want };
      this.headV = { x: 0, y: 0 };
      this.squint = 0;
      this.breathe = 0;
      return { eye: this.eye, blink };
    }

    if (inp.state === "driving") {
      // At the computer the eyes stay on its own cursor: smooth pursuit.
      const k = 1 - Math.exp(-dt / 0.04);
      this.eye = { x: this.eye.x + (want.x - this.eye.x) * k, y: this.eye.y + (want.y - this.eye.y) * k };
      this.fix = { ...want };
      this.jump = null;
    } else {
      const d = Math.hypot(want.x - this.fix.x, want.y - this.fix.y);
      const ready = now - this.lastJump > this.refract;
      const real = this.jump && !this.jump.micro;
      const inFlight = real && now - this.jump!.t0 < this.jump!.dur;
      if (!inFlight && ready && d > SACCADE.big) {
        blink = this.saccade(now, want);
      } else if (!inFlight && d > SACCADE.small) {
        if (this.pendingSince === null) this.pendingSince = now;
        else if (ready && now - this.pendingSince >= SACCADE.dwell) blink = this.saccade(now, want);
      } else if (d <= SACCADE.small) {
        this.pendingSince = null;
      }
      // Micro-saccades: only while holding still on something.
      if (!this.jump && now >= this.nextMicro && now - this.lastJump > MICRO.quietFor) {
        const a = this.r(MICRO.ampMin, MICRO.ampMax);
        const ang = this.r(0, Math.PI * 2);
        const to = { x: this.fix.x + Math.cos(ang) * a, y: this.fix.y + Math.sin(ang) * a * 0.6 };
        this.jump = { from: { ...this.eye }, to, t0: now, dur: MICRO.dur, over: 0, micro: true };
        this.nextMicro = now + this.r(MICRO.gapMin, MICRO.gapMax);
      }
      if (this.jump) {
        const j = this.jump;
        const x = (now - j.t0) / j.dur;
        const settleX = j.over > 0 ? SACCADE.settle / j.dur : 0;
        const p = saccadeCurve(x, j.over, settleX);
        this.eye = { x: j.from.x + (j.to.x - j.from.x) * p, y: j.from.y + (j.to.y - j.from.y) * p };
        if (x >= 1 + settleX) {
          this.eye = { ...j.to };
          this.jump = null;
        }
      }
    }

    // The head: a slower spring after the fixation (not the micro wobble).
    if (this.style === "head") {
      const tx = this.fix.x * HEAD.follow;
      const ty = this.fix.y * HEAD.follow;
      const c = 2 * HEAD.zeta * Math.sqrt(HEAD.k);
      const steps = Math.max(1, Math.ceil(dt / (1 / 120)));
      const h = dt / steps;
      for (let i = 0; i < steps; i++) {
        this.headV.x += (HEAD.k * (tx - this.head.x) - c * this.headV.x) * h;
        this.headV.y += (HEAD.k * (ty - this.head.y) - c * this.headV.y) * h;
        this.head.x += this.headV.x * h;
        this.head.y += this.headV.y * h;
      }
      const sq = this.mode === "near" ? 1 : 0;
      this.squintTo = sq;
      this.squint += (sq - this.squint) * (1 - Math.exp(-dt / ATTENTION.squintTau));
      if (Math.abs(sq - this.squint) < 0.004) this.squint = sq;
      this.breathe = Math.sin((now * 2 * Math.PI) / HEAD.breathPeriod);
    } else {
      this.head = { ...this.eye };
    }
    return { eye: this.eye, blink };
  }

  /** Starts a real saccade; true when it should come with a blink. */
  private saccade(now: number, to: Vec): boolean {
    const from = { ...this.eye };
    const dx = to.x - this.fix.x;
    const dy = to.y - this.fix.y;
    const amp = Math.hypot(dx, dy);
    const k = clamp(amp / SACCADE.durFullAt, 0, 1);
    // Sampled, not continuous: the jump aims at where the target is now and
    // holds there until the next one.
    this.jump = {
      from,
      to: { ...to },
      t0: now,
      dur: SACCADE.durMin + (SACCADE.durMax - SACCADE.durMin) * k,
      over: SACCADE.overMin + (SACCADE.overMax - SACCADE.overMin) * k,
      micro: false,
    };
    this.fix = { ...to };
    this.lastJump = now;
    this.refract = SACCADE.refract + this.r(0, SACCADE.refractJitter);
    this.pendingSince = null;
    this.nextMicro = Math.max(this.nextMicro, now + MICRO.quietFor + this.r(MICRO.gapMin, MICRO.gapMax) * 0.5);
    const dir = amp > 0 ? { x: dx / amp, y: dy / amp } : { x: 0, y: 0 };
    const reverse = dir.x * this.lastDir.x + dir.y * this.lastDir.y < -0.3;
    this.lastDir = dir;
    const big = amp >= SACCADE.blinkAmp || (reverse && amp >= SACCADE.blinkReverseAmp);
    if (big && now - this.lastBlinkAsk > SACCADE.blinkCooldown && this.rand() < SACCADE.blinkChance) {
      this.lastBlinkAsk = now;
      return true;
    }
    return false;
  }

  /** True while something visible is in motion and wants every frame. */
  busy(now: number): boolean {
    if (this.jump) return true;
    if (this.style === "head") {
      // 0.005 of a full turn is under a tenth of a pixel at 84 px.
      const e = HEAD.follow;
      if (Math.abs(this.fix.x * e - this.head.x) > 0.005 || Math.abs(this.fix.y * e - this.head.y) > 0.005) return true;
      if (Math.abs(this.headV.x) > 0.03 || Math.abs(this.headV.y) > 0.03) return true;
      if (this.surpriseLevel(now) > 0) return true;
      if (Math.abs(this.squintTo - this.squint) > 0.002) return true;
    }
    return false;
  }

  /** Moments the loop must wake up for even when nothing moves. */
  wakeTimes(): number[] {
    const out = [this.nextMicro];
    if (this.pendingSince !== null) out.push(this.pendingSince + SACCADE.dwell, this.lastJump + this.refract);
    if (this.style === "head") {
      out.push(this.lastMeaningful + ATTENTION.boredAfter + 0.01, this.lastMeaningful + ATTENTION.deepAfter + 0.01);
      if (this.bored) out.push(this.bored.until);
    }
    return out;
  }

  /** A capped frame rate for motion that never stops (breathing). */
  ambientFps(reduced: boolean): number {
    return this.style === "head" && !reduced ? HEAD.breathFps : 0;
  }

  frame(now: number): RigFrame {
    return {
      style: this.style,
      head: { ...this.head },
      breathe: this.breathe,
      surprise: this.style === "head" ? this.surpriseLevel(now) : 0,
      squint: this.squint,
      mode: this.mode,
      saccading: this.jump !== null && !this.jump.micro,
    };
  }

  /** For tests: the current fixation. */
  get fixation(): Vec {
    return { ...this.fix };
  }

  /** For tests and the lab: seconds since the last step. */
  idleFor(now: number): number {
    return now - this.lastStep;
  }
}

// ---------- placement: where each bean and the mouth go ----------

export interface Place {
  x: number;
  y: number;
  /** Width and height scale. */
  sx: number;
  sy: number;
}

export interface PillPlacement {
  /** Left eye, right eye. */
  eyes: [Place, Place];
  mouth: Place;
  /** Highlight slide (gloss), in bean fractions: x + = toward the screen right. */
  hl: Vec;
  /** The turn used for shading (-1..1). */
  turn: Vec;
}

export interface PillGeom {
  d: number;
  hx: number;
  hy: number;
  /** The mouth's anchor at rest (face units). */
  mouthX: number;
  mouthY: number;
}

/** Cozmo perspective for a gaze (u, v): the pair moves toward the gaze side,
 * the near eye grows, the far eye shrinks and narrows, the gap closes. */
function cozmo(u: number, v: number, g: PillGeom): PillPlacement {
  u = clamp(u, -1, 1);
  v = clamp(v, -1, 1);
  const C = COZMO;
  const cx = u * C.shiftX * g.hx;
  const cy = v * C.shiftY * g.hy;
  const half = g.d * (1 - C.squeeze * Math.abs(u));
  const up = v < 0 ? -v : 0;
  const down = v > 0 ? v : 0;
  const vh = 1 + C.upH * up + C.downH * down;
  const vw = 1 + C.upW * up + C.downW * down;
  const eye = (s: -1 | 1): Place => {
    const a = s * u; // + toward the gaze side
    const sy = (1 + a * (a > 0 ? C.nearH : C.farH)) * vh;
    const sx = (1 + a * (a > 0 ? C.nearW : C.farW)) * vw;
    return { x: cx + s * half, y: cy, sx, sy };
  };
  return {
    eyes: [eye(-1), eye(1)],
    mouth: {
      x: g.mouthX + cx * C.mouthFollow,
      y: g.mouthY + cy * 0.8,
      sx: 1 - C.mouthSquashX * Math.abs(u),
      sy: 1 - C.mouthSquashY * Math.abs(v),
    },
    hl: { x: -u * GLOSS.slide, y: -v * GLOSS.slideY },
    turn: { x: u, y: v },
  };
}

/** A point of the face plane on a sphere of radius `R`, turned by yaw / pitch
 * (radians): its new position, the local foreshortening and the perspective
 * size (nearer = bigger). */
export function onSphere(x: number, y: number, R: number, yaw: number, pitch: number, camera: number) {
  const th = Math.asin(clamp(x / R, -0.98, 0.98));
  const ph = Math.asin(clamp(y / R, -0.98, 0.98));
  const a = clamp(th + yaw, -1.45, 1.45);
  const b = clamp(ph + pitch, -1.45, 1.45);
  const z = R * Math.cos(a) * Math.cos(b);
  const D = camera * R;
  const p = D / (D + R - z);
  return {
    x: R * Math.sin(a) * p,
    y: R * Math.sin(b) * p,
    sx: (Math.cos(a) / Math.cos(th)) * p,
    sy: (Math.cos(b) / Math.cos(ph)) * p,
  };
}

/** Where the beans and the mouth go for a style, the eyes' gaze and the head. */
export function placePill(style: MotionStyle, eye: Vec, head: Vec, g: PillGeom): PillPlacement {
  if (style !== "head") return cozmo(eye.x, eye.y, g);
  // Head: Cozmo perspective from the eyes' offset inside the head (and a
  // share of the head turn), then the whole plane turned on a sphere.
  const rel = { x: eye.x - head.x, y: eye.y - head.y };
  const S = SPHERE;
  const base = cozmo(rel.x * S.relGain + head.x * S.headGain, rel.y * S.relGain + head.y * S.headGain, g);
  const R = S.radius * g.d;
  const yaw = (clamp(head.x, -1.2, 1.2) * S.yaw * Math.PI) / 180;
  const pitch = (clamp(head.y, -1.2, 1.2) * S.pitch * Math.PI) / 180;
  const turn = (p: Place): Place => {
    const q = onSphere(p.x, p.y, R, yaw, pitch, S.camera);
    return { x: q.x, y: q.y, sx: p.sx * q.sx, sy: p.sy * q.sy };
  };
  const tx = clamp(eye.x, -1, 1);
  const ty = clamp(eye.y, -1, 1);
  return {
    eyes: [turn(base.eyes[0]), turn(base.eyes[1])],
    mouth: turn(base.mouth),
    hl: { x: -tx * GLOSS.slide, y: -ty * GLOSS.slideY },
    turn: { x: tx, y: ty },
  };
}
