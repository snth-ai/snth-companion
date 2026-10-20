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
//   smooth The same look and character as head, tuned for fluid motion (the
//          owner liked head but found it jerky): slow pointer motion is
//          followed by smooth pursuit (a critically damped glide), only big
//          or fast shifts saccade, and those ease in and out over 140-220 ms
//          with a 3% overshoot; the head spring is near critical, breath is
//          drawn every frame, startle and squint ease in and out, and the
//          squint waits for the pointer to dwell near the face. Its numbers
//          are live-tunable (SmoothTuning), the lab has sliders for them.
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

export type MotionStyle = "classic" | "cozmo" | "gloss" | "head" | "smooth";
export const MOTION_STYLES: readonly MotionStyle[] = ["classic", "cozmo", "gloss", "head", "smooth"];

/** The motion pill eyes get when the caller does not choose one. The owner
 * picked smooth (C2) in the face lab; set this back to "classic" to roll
 * the island back to the sliding eyes. */
export const DEFAULT_PILL_MOTION: MotionStyle = "smooth";

/** Styles with the head layer (sphere turn, breath, attention). */
export const HEAD_STYLES: ReadonlySet<MotionStyle> = new Set(["head", "smooth"]);

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
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
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
  /** Smooth style: the live-tuned turn (degrees), Cozmo share, breath size. */
  yaw?: number;
  pitch?: number;
  cozmoGain?: number;
  breathScale?: number;
  breathDrift?: number;
}

/** What FaceSim needs from a rig (GazeRig for cozmo / gloss / head,
 * SmoothRig for smooth). */
export interface Rig {
  readonly style: MotionStyle;
  noteCursor(g: Vec | null, now: number, agent?: boolean): void;
  step(now: number, dt: number, inp: RigInput): RigOut;
  busy(now: number): boolean;
  wakeTimes(): number[];
  ambientFps(reduced: boolean): number;
  frame(now: number): RigFrame;
  /** Live tuning (smooth style only). */
  setTuning?(t: Partial<SmoothTuning> | undefined): void;
  /** How much of the pill eye's after-blink bounce to keep (1 = all). */
  reboundGain?(): number;
  /** Blink length multiplier (1 = the shared timing). */
  blinkScale?(): number;
}

export interface RigInput {
  /** Where the state wants the eyes (targetPose gx / gy). */
  want: Vec;
  /** The pointer as a gaze vector, or null. */
  cursor: Vec | null;
  state: FaceState;
  reduced: boolean;
  /** What the host knows about the pointer (the island; see FaceAttention). */
  attention?: FaceAttention;
}

/** What the host knows about the pointer and the person, beyond the gaze
 * vector. The island fills it in; elsewhere (editor, demo, lab) it is left
 * out and the rig judges "near" by distance. */
export interface FaceAttention {
  /** The pointer is on the host's own shape (or right below it). When set,
   * it replaces the distance test for the happy squint. */
  near?: boolean;
  /** The person is busy with the host (hover, typing, a hold, a request, an
   * answer showing): no boredom, no glancing away. */
  engaged?: boolean;
  /** The gaze follows the synth's own pointer (it drives the computer):
   * just follow, no startle, no squint, no boredom. */
  agent?: boolean;
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

export class GazeRig implements Rig {
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
export function placePill(style: MotionStyle, eye: Vec, head: Vec, g: PillGeom, rig?: RigFrame): PillPlacement {
  if (!HEAD_STYLES.has(style)) return cozmo(eye.x, eye.y, g);
  const S = SPHERE;
  let base: PillPlacement;
  let yawDeg = S.yaw;
  let pitchDeg = S.pitch;
  if (style === "smooth") {
    // Smooth: the Cozmo perspective follows the eyes themselves (a share of
    // it), so a saccade changes it once instead of pumping it up while the
    // head lags and back down as the head catches up.
    const k = rig?.cozmoGain ?? SMOOTH.cozmoGain;
    base = cozmo(eye.x * k, eye.y * k, g);
    yawDeg = rig?.yaw ?? SMOOTH_DEFAULTS.turn;
    pitchDeg = rig?.pitch ?? SMOOTH_DEFAULTS.turn * SMOOTH.pitchOfTurn;
  } else {
    // Head: Cozmo perspective from the eyes' offset inside the head (and a
    // share of the head turn), then the whole plane turned on a sphere.
    const rel = { x: eye.x - head.x, y: eye.y - head.y };
    base = cozmo(rel.x * S.relGain + head.x * S.headGain, rel.y * S.relGain + head.y * S.headGain, g);
  }
  const R = S.radius * g.d;
  const yaw = (clamp(head.x, -1.2, 1.2) * yawDeg * Math.PI) / 180;
  const pitch = (clamp(head.y, -1.2, 1.2) * pitchDeg * Math.PI) / 180;
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

// ---------- smooth: the fluid head style (C2) ----------

/** The live-tunable part of the smooth style (the lab's sliders). */
export interface SmoothTuning {
  /** 0..1: how much the gaze glides instead of jumping. Higher = a larger
   * offset still glides (saccade threshold), slower glides and longer jumps. */
  smoothness: number;
  /** Head spring: stiffness (1/s^2) and damping ratio (1 = critical). */
  headK: number;
  headZeta: number;
  /** Max head turn sideways, degrees (up / down is 0.7 of it). */
  turn: number;
  /** Breath size, 1 = default, 0 = none (and the loop may rest). */
  breath: number;
  /** Boredom: glance away from a resting pointer after `boredAfter` s. */
  bored: boolean;
  boredAfter: number;
}

export const SMOOTH_DEFAULTS: SmoothTuning = {
  smoothness: 0.5,
  headK: 30,
  headZeta: 0.85,
  turn: 16,
  breath: 1,
  bored: true,
  boredAfter: 4,
};

/** Fixed numbers of the smooth style (seconds, gaze units). */
export const SMOOTH = {
  /** Eye-to-target distance that triggers a saccade, at smoothness 0 and 1. */
  saccadeAt: [0.2, 0.6] as const,
  /** Pursuit glide: natural frequency (rad/s) at smoothness 0 and 1, critically damped. */
  pursuitW: [26, 10] as const,
  /** Saccade length for a short and a full (amplitude >= durFullAt) jump, at smoothness 0 and 1. */
  durShort: [0.1, 0.18] as const,
  durLong: [0.16, 0.28] as const,
  durFullAt: 1.2,
  overshoot: 0.03,
  settle: 0.09,
  /** After a saccade lands, pursuit only (no new jump) for this long. */
  refract: 0.1,
  blinkAmp: 0.9,
  blinkChance: 0.35,
  blinkCooldown: 2,
  headFollow: 0.72,
  pitchOfTurn: 0.7,
  cozmoGain: 0.75,
  breathPeriod: 4.6,
  breathScale: 0.006,
  breathDrift: 0.22,
  /** Near: enter / leave radius (hysteresis), dwell before the squint, look-down. */
  nearIn: 0.22,
  nearOut: 0.3,
  nearDwell: 0.4,
  nearLookDown: 0.35,
  /** Squint: critically damped toward on / off at this frequency (rad/s). */
  squintW: 7,
  flick: 4.5,
  flickTau: 0.05,
  startleRise: 0.18,
  startleHold: 0.12,
  startleFall: 0.5,
  startleGain: 0.75,
  startleCooldown: 2.5,
  /** Keep this much of the pill eye's bounce after a blink. */
  rebound: 0.35,
  /** Blinks a little slower than the shared timing, softer. */
  blinkScale: 1.2,
  /** The pointer arrives in samples (the island shell sends about 30 a
   * second while it moves); pursuit follows a copy smoothed over about one
   * sample, so the glide has no 30 Hz ripple. */
  targetTau: 0.035,
  /** Settled (only breathing): this many frames a second. The breath moves
   * under 0.6 px per second at 84 px, so a frame is a 0.03 px step. */
  breathFps: 20,
};

/** The numbers a smoothness value stands for. */
export function smoothDerived(t: SmoothTuning) {
  const s = clamp(t.smoothness, 0, 1);
  return {
    saccadeAt: lerp(SMOOTH.saccadeAt[0], SMOOTH.saccadeAt[1], s),
    pursuitW: lerp(SMOOTH.pursuitW[0], SMOOTH.pursuitW[1], s),
    durShort: lerp(SMOOTH.durShort[0], SMOOTH.durShort[1], s),
    durLong: lerp(SMOOTH.durLong[0], SMOOTH.durLong[1], s),
  };
}

/** Cubic Hermite from p0 (velocity v0, per second) to p1 (velocity 0) over T seconds, at x 0..1. */
function hermite(p0: number, v0: number, p1: number, T: number, x: number): number {
  const x2 = x * x;
  const x3 = x2 * x;
  return (2 * x3 - 3 * x2 + 1) * p0 + (x3 - 2 * x2 + x) * T * v0 + (-2 * x3 + 3 * x2) * p1;
}

interface Glide {
  p0: Vec;
  v0: Vec;
  /** Where it lands (past the target by the overshoot), and the target. */
  over: Vec;
  to: Vec;
  t0: number;
  dur: number;
}

export class SmoothRig implements Rig {
  readonly style: MotionStyle = "smooth";
  private rand: () => number;
  private t: SmoothTuning = { ...SMOOTH_DEFAULTS };
  private d = smoothDerived(SMOOTH_DEFAULTS);

  private eye: Vec;
  private eyeV: Vec = { x: 0, y: 0 };
  private want: Vec;
  /** The target pursuit follows: `want` smoothed over about one sample. */
  private aim: Vec;
  private sac: Glide | null = null;
  private sacEnd = -Infinity;
  private lastBlinkAsk = -Infinity;

  private head: Vec;
  private headV: Vec = { x: 0, y: 0 };

  private cursorPrev: Vec | null = null;
  private cursorPrevAt = -Infinity;
  private speed = 0;
  private lastMeaningful: number;
  private meaningfulFrom: Vec | null = null;
  private surpriseAt = -Infinity;
  private nearSince: number | null = null;
  private nearOn = false;
  private squint = 0;
  private squintV = 0;
  private bored: { away: boolean; until: number; spot: Vec } | null = null;
  private mode: RigFrame["mode"] = "track";
  private breathe = 0;

  constructor(now: number, start: Vec, rand: () => number = Math.random) {
    this.rand = rand;
    this.eye = { ...start };
    this.want = { ...start };
    this.aim = { ...start };
    this.head = { x: start.x * SMOOTH.headFollow, y: start.y * SMOOTH.headFollow };
    this.lastMeaningful = now;
  }

  private r(lo: number, hi: number): number {
    return lo + this.rand() * (hi - lo);
  }

  setTuning(t: Partial<SmoothTuning> | undefined): void {
    this.t = { ...SMOOTH_DEFAULTS, ...(t ?? {}) };
    this.d = smoothDerived(this.t);
  }

  reboundGain(): number {
    return SMOOTH.rebound;
  }

  blinkScale(): number {
    return SMOOTH.blinkScale;
  }

  noteCursor(g: Vec | null, now: number, agent = false): void {
    if (!g) {
      this.cursorPrev = null;
      return;
    }
    const prev = this.cursorPrev;
    const dt = now - this.cursorPrevAt;
    // Speed only between samples of one movement: the shell sends nothing
    // while the pointer rests, so the first sample after a pause (dt over
    // 0.15 s) starts a new movement instead of counting the gap as a jump.
    if (prev && dt > 0.004 && dt < 0.15) {
      const v = Math.hypot(g.x - prev.x, g.y - prev.y) / dt;
      this.speed += (v - this.speed) * (1 - Math.exp(-dt / SMOOTH.flickTau));
    } else {
      this.speed = 0;
    }
    if (agent) {
      // The synth's own pointer: follow it, nothing to react to.
      this.speed = 0;
      this.cursorPrev = { ...g };
      this.cursorPrevAt = now;
      this.lastMeaningful = now;
      this.meaningfulFrom = { ...g };
      this.bored = null;
      return;
    }
    this.cursorPrev = { ...g };
    this.cursorPrevAt = now;
    const from = this.meaningfulFrom;
    if (!from || Math.hypot(g.x - from.x, g.y - from.y) > ATTENTION.meaningful) {
      this.lastMeaningful = now;
      this.meaningfulFrom = { ...g };
      this.bored = null;
    }
    if (this.speed > SMOOTH.flick && now - this.surpriseAt > SMOOTH.startleCooldown) this.surpriseAt = now;
  }

  /** Startle 0..1: eases in, holds a moment, eases out. */
  surpriseLevel(now: number): number {
    const e = now - this.surpriseAt;
    const { startleRise: a, startleHold: h, startleFall: b } = SMOOTH;
    if (e < 0 || e > a + h + b) return 0;
    const k = e < a ? smooth(e / a) : e < a + h ? 1 : 1 - smooth((e - a - h) / b);
    return k * SMOOTH.startleGain;
  }

  private attend(now: number, inp: RigInput): Vec {
    const att = inp.attention;
    if (inp.reduced || inp.state !== "idle" || att?.agent) {
      this.mode = inp.state === "idle" ? "track" : "state";
      this.bored = null;
      this.nearSince = null;
      this.nearOn = false;
      return inp.want;
    }
    // Near, with a dwell before it counts and a wider radius to leave, so a
    // pointer passing by does not trigger it and the edge does not flicker.
    // A host that knows its own shape (the island) says so instead.
    const c = inp.cursor;
    const dist = c ? len(c) : Infinity;
    const hostNear = att?.near;
    const isIn = hostNear !== undefined ? hostNear && c !== null : dist < SMOOTH.nearIn;
    const isOut = hostNear !== undefined ? !hostNear || c === null : dist > SMOOTH.nearOut;
    if (this.nearOn) {
      if (isOut) {
        this.nearOn = false;
        this.nearSince = null;
      }
    } else if (isIn) {
      if (this.nearSince === null) this.nearSince = now;
      else if (now - this.nearSince >= SMOOTH.nearDwell) this.nearOn = true;
    } else {
      this.nearSince = null;
    }
    if (this.nearOn) {
      this.mode = "near";
      this.bored = null;
      return { x: inp.want.x, y: clamp(inp.want.y + SMOOTH.nearLookDown, -1, 1) };
    }
    const rest = now - this.lastMeaningful;
    if (!this.t.bored || rest < this.t.boredAfter || att?.engaged) {
      this.mode = "track";
      this.bored = null;
      return inp.want;
    }
    const deep = rest > this.t.boredAfter + ATTENTION.deepAfter - ATTENTION.boredAfter;
    if (!this.bored || now >= this.bored.until) {
      const away = !this.bored || !this.bored.away;
      if (away) {
        let spot: Vec;
        if (deep && this.rand() < 0.4) spot = { x: this.r(-0.1, 0.1), y: this.r(-0.05, 0.12) };
        else {
          const side = inp.want.x > 0.15 ? -1 : inp.want.x < -0.15 ? 1 : this.rand() < 0.5 ? -1 : 1;
          spot = { x: side * this.r(0.35, 0.85), y: this.r(-0.4, 0.25) };
        }
        this.bored = { away: true, until: now + this.r(ATTENTION.awayMin, ATTENTION.awayMax) * (deep ? 2 : 1.2), spot };
      } else {
        this.bored = { away: false, until: now + this.r(ATTENTION.backMin, ATTENTION.backMax) * (deep ? 0.8 : 1.2), spot: inp.want };
      }
    }
    this.mode = deep ? "deep" : "bored";
    return this.bored.away ? this.bored.spot : inp.want;
  }

  step(now: number, dt: number, inp: RigInput): RigOut {
    let blink = false;
    const want = this.attend(now, inp);
    this.want = want;
    if (inp.reduced) {
      this.eye = { ...want };
      this.aim = { ...want };
      this.eyeV = { x: 0, y: 0 };
      this.sac = null;
      this.head = { x: want.x * SMOOTH.headFollow, y: want.y * SMOOTH.headFollow };
      this.headV = { x: 0, y: 0 };
      this.squint = 0;
      this.squintV = 0;
      this.breathe = 0;
      return { eye: this.eye, blink };
    }
    const steps = Math.max(1, Math.ceil(dt / (1 / 240)));
    const h = dt / steps;

    // The aim: the target smoothed over about one pointer sample.
    const ak = 1 - Math.exp(-dt / SMOOTH.targetTau);
    this.aim = { x: this.aim.x + (want.x - this.aim.x) * ak, y: this.aim.y + (want.y - this.aim.y) * ak };
    const following = inp.state === "driving" || Boolean(inp.attention?.agent);
    if (!this.sac) {
      const dist = Math.hypot(want.x - this.eye.x, want.y - this.eye.y);
      if (!following && dist > this.d.saccadeAt && now - this.sacEnd > SMOOTH.refract) blink = this.startSaccade(now, want);
    }
    if (this.sac) {
      const j = this.sac;
      const x = (now - j.t0) / j.dur;
      const prev = { ...this.eye };
      if (x < 1) {
        this.eye = { x: hermite(j.p0.x, j.v0.x, j.over.x, j.dur, x), y: hermite(j.p0.y, j.v0.y, j.over.y, j.dur, x) };
      } else {
        const x2 = (now - j.t0 - j.dur) / SMOOTH.settle;
        if (x2 < 1) {
          const k = smooth(x2);
          this.eye = { x: j.over.x + (j.to.x - j.over.x) * k, y: j.over.y + (j.to.y - j.over.y) * k };
        } else {
          this.eye = { ...j.to };
          this.sac = null;
          this.sacEnd = now;
          // Pursuit picks up from where the jump landed.
          this.aim = { ...j.to };
        }
      }
      this.eyeV = this.sac && dt > 0 ? { x: (this.eye.x - prev.x) / dt, y: (this.eye.y - prev.y) / dt } : { x: 0, y: 0 };
    } else {
      // Smooth pursuit: a critically damped glide after the aim; faster
      // after the synth's own pointer.
      const w = following ? 30 : this.d.pursuitW;
      const aim = this.aim;
      for (let i = 0; i < steps; i++) {
        this.eyeV.x += (w * w * (aim.x - this.eye.x) - 2 * w * this.eyeV.x) * h;
        this.eyeV.y += (w * w * (aim.y - this.eye.y) - 2 * w * this.eyeV.y) * h;
        this.eye.x += this.eyeV.x * h;
        this.eye.y += this.eyeV.y * h;
      }
    }

    // The head follows the eyes on its own, slower spring.
    const k = this.t.headK;
    const c = 2 * this.t.headZeta * Math.sqrt(k);
    const tx = this.eye.x * SMOOTH.headFollow;
    const ty = this.eye.y * SMOOTH.headFollow;
    for (let i = 0; i < steps; i++) {
      this.headV.x += (k * (tx - this.head.x) - c * this.headV.x) * h;
      this.headV.y += (k * (ty - this.head.y) - c * this.headV.y) * h;
      this.head.x += this.headV.x * h;
      this.head.y += this.headV.y * h;
    }

    // Squint: critically damped toward on / off, so it eases both ways.
    const sq = this.mode === "near" ? 1 : 0;
    const sw = SMOOTH.squintW;
    for (let i = 0; i < steps; i++) {
      this.squintV += (sw * sw * (sq - this.squint) - 2 * sw * this.squintV) * h;
      this.squint += this.squintV * h;
    }
    this.breathe = this.t.breath > 0 ? Math.sin((now * 2 * Math.PI) / SMOOTH.breathPeriod) : 0;
    return { eye: this.eye, blink };
  }

  private startSaccade(now: number, to: Vec): boolean {
    const amp = Math.hypot(to.x - this.eye.x, to.y - this.eye.y);
    const k = clamp(amp / SMOOTH.durFullAt, 0, 1);
    const ov = SMOOTH.overshoot;
    this.sac = {
      p0: { ...this.eye },
      v0: { ...this.eyeV },
      over: { x: to.x + (to.x - this.eye.x) * ov, y: to.y + (to.y - this.eye.y) * ov },
      to: { ...to },
      t0: now,
      dur: lerp(this.d.durShort, this.d.durLong, k),
    };
    if (amp >= SMOOTH.blinkAmp && now - this.lastBlinkAsk > SMOOTH.blinkCooldown && this.rand() < SMOOTH.blinkChance) {
      this.lastBlinkAsk = now;
      return true;
    }
    return false;
  }

  busy(now: number): boolean {
    // Breathing alone is not "busy": it runs at breathFps (ambientFps).
    if (this.sac) return true;
    if (Math.abs(this.aim.x - this.eye.x) > 0.002 || Math.abs(this.aim.y - this.eye.y) > 0.002) return true;
    if (Math.abs(this.want.x - this.eye.x) > 0.002 || Math.abs(this.want.y - this.eye.y) > 0.002) return true;
    if (Math.abs(this.eyeV.x) > 0.01 || Math.abs(this.eyeV.y) > 0.01) return true;
    const f = SMOOTH.headFollow;
    if (Math.abs(this.eye.x * f - this.head.x) > 0.003 || Math.abs(this.eye.y * f - this.head.y) > 0.003) return true;
    if (Math.abs(this.headV.x) > 0.01 || Math.abs(this.headV.y) > 0.01) return true;
    if (this.surpriseLevel(now) > 0) return true;
    if (Math.abs((this.mode === "near" ? 1 : 0) - this.squint) > 0.002 || Math.abs(this.squintV) > 0.01) return true;
    return false;
  }

  wakeTimes(): number[] {
    const out: number[] = [];
    if (this.t.bored) out.push(this.lastMeaningful + this.t.boredAfter + 0.01);
    if (this.bored) out.push(this.bored.until);
    if (this.nearSince !== null && !this.nearOn) out.push(this.nearSince + SMOOTH.nearDwell + 0.01);
    return out;
  }

  ambientFps(reduced: boolean): number {
    return this.t.breath > 0 && !reduced ? SMOOTH.breathFps : 0;
  }

  frame(now: number): RigFrame {
    const b = this.t.breath;
    return {
      style: "smooth",
      head: { ...this.head },
      breathe: this.breathe,
      surprise: this.surpriseLevel(now),
      squint: clamp(this.squint, 0, 1),
      mode: this.mode,
      saccading: this.sac !== null,
      yaw: this.t.turn,
      pitch: this.t.turn * SMOOTH.pitchOfTurn,
      cozmoGain: SMOOTH.cozmoGain,
      breathScale: SMOOTH.breathScale * b,
      breathDrift: SMOOTH.breathDrift * b,
    };
  }
}

/** The rig for a style (classic has none). */
export function makeRig(style: MotionStyle, now: number, start: Vec, tuning?: Partial<SmoothTuning>): Rig | null {
  if (style === "classic") return null;
  if (style === "smooth") {
    const r = new SmoothRig(now, start);
    r.setTuning(tuning);
    return r;
  }
  return new GazeRig(style, now, start);
}
