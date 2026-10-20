// The synth's face on the island: SVG on a black background (the island
// draws the black; the face itself is transparent).
//
// Scale: one continuous mapping from the height in px to the face units it
// shows (see faceView), so the eyes never jump when the island opens: a
// closed island (32-37 px) shows a crop around the eyes, an open one
// (about 80 px and up) the whole head. Detail (brows, mouth, accessories)
// fades in above COMPACT_BELOW. Small symbols beside the eyes say what the
// synth is doing where the eyes alone cannot: a cursor (at the computer),
// "?" (needs permission), "!" (problem), a three-bar meter (listening).
// See motion.ts for how it moves and what it costs.

import { useEffect, useId, useMemo, useRef, useState, type CSSProperties, type ReactElement } from "react";
import { darken, drain, lighten, luminance, mix, visibleOnBlack } from "./color";
import { PILL_EYE, eyeGeom, f, heart, pillPath, softStar, sparkle } from "./geometry";
import { EMOTE_LENGTH, FaceSim, HERO_TIME, LID_REST, nodCurve, stillFrame, type Frame, type Vec } from "./motion";
import { clampFace, type FaceConfig, type FaceState, type IdleEmote } from "./schema";

/** Below this height (px) brows, mouth and accessories are hidden. */
export const COMPACT_BELOW = 48;

/** The two ends of the scale: a closed island and an open one. Between
 * them everything is interpolated by height. */
const SMALL = { size: 34, h: 82, cy: 0, aspect: 2.05 };
const LARGE = { size: 80, h: 114, cy: -5, aspect: 1.4 };

export interface FaceView {
  x: number;
  y: number;
  w: number;
  h: number;
  /** Face units per screen pixel. */
  px: number;
}

/** The part of the face world shown at a height (px). */
export function faceView(size: number): FaceView {
  const k = Math.min(1, Math.max(0, (size - SMALL.size) / (LARGE.size - SMALL.size)));
  const h = SMALL.h + (LARGE.h - SMALL.h) * k;
  const cy = SMALL.cy + (LARGE.cy - SMALL.cy) * k;
  const w = h * (SMALL.aspect + (LARGE.aspect - SMALL.aspect) * k);
  return { x: -w / 2, y: cy - h / 2, w, h, px: h / size };
}

/** Width / height of the face at a height (px). */
export function faceAspect(size: number): number {
  const v = faceView(size);
  return v.w / v.h;
}

/** The width in px the face takes at a height (px). */
export function faceWidth(size: number): number {
  return Math.round(size * faceAspect(size));
}

export interface FaceProps {
  /** The look; null or broken parts fall back to the classic preset (clamped, never throws). */
  config: FaceConfig | null | undefined;
  state?: FaceState;
  /** Where to look: a vector from the face centre, x -1 (left) .. 1 (right),
   * y -1 (up) .. 1 (down). null = no pointer known (looks slightly down). */
  gaze?: Vec | null;
  /** Microphone level 0..1, used while listening. */
  level?: number;
  /** Height in px. Width follows (faceWidth). */
  size?: number;
  /** Defaults to the system setting. */
  reducedMotion?: boolean;
  /** Hide brows, mouth and accessories; defaults to size < COMPACT_BELOW. */
  compact?: boolean;
  /** Draw one settled pose and never animate (thumbnails, screenshots). */
  still?: boolean;
  /** Idle with no gaze change for motion.sleepy_after_min falls asleep by itself. Default true. */
  autoSleep?: boolean;
  /** Plays an idle emote when `nonce` changes (editor preview). */
  emote?: { kind: IdleEmote; nonce: number } | null;
  /** With `still`: a blink closure 0..1 to draw (review screenshots). */
  blink?: number;
  label?: string;
  className?: string;
  style?: CSSProperties;
}

function usePrefersReducedMotion(): boolean {
  const query = "(prefers-reduced-motion: reduce)";
  const [value, setValue] = useState(() => typeof matchMedia === "function" && matchMedia(query).matches);
  useEffect(() => {
    if (typeof matchMedia !== "function") return;
    const mq = matchMedia(query);
    const on = () => setValue(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return value;
}

export function Face(props: FaceProps) {
  const { state = "idle", level = 0, size = 36, still = false, autoSleep = true } = props;
  const face = useMemo(() => clampFace(props.config ?? undefined), [props.config]);
  const systemReduced = usePrefersReducedMotion();
  const reduced = props.reducedMotion ?? systemReduced;
  const compact = props.compact ?? size < COMPACT_BELOW;
  const gx = props.gaze ? props.gaze.x : null;
  const gy = props.gaze ? props.gaze.y : null;
  const gaze = useMemo(() => (gx === null || gy === null ? null : { x: gx, y: gy }), [gx, gy]);
  const uid = useId().replace(/[^a-zA-Z0-9_-]/g, "");

  const input = useMemo(() => ({ face, state, gaze, level, reduced, autoSleep, compact }), [face, state, gaze, level, reduced, autoSleep, compact]);
  const inputRef = useRef(input);
  inputRef.current = input;
  const simRef = useRef<FaceSim | null>(null);
  const [live, setLive] = useState<Frame | null>(null);

  useEffect(() => {
    if (still) return;
    const sim = new FaceSim(inputRef.current, setLive);
    simRef.current = sim;
    setLive(sim.frame());
    sim.start();
    return () => {
      sim.stop();
      simRef.current = null;
    };
  }, [still]);

  useEffect(() => {
    simRef.current?.update(input);
  }, [input]);

  const emoteKind = props.emote?.kind;
  const emoteNonce = props.emote?.nonce;
  useEffect(() => {
    if (emoteKind && emoteNonce !== undefined) simRef.current?.playEmote(emoteKind);
  }, [emoteKind, emoteNonce]);

  const stillBlink = still ? (props.blink ?? 0) : 0;
  const stillShot = useMemo(
    () =>
      still || !live
        ? stillFrame(face, state, gaze, level, compact, still && emoteKind ? { kind: emoteKind, t: EMOTE_LENGTH[emoteKind] * 0.45, side: 1 } : null, stillBlink)
        : null,
    [still, live, face, state, gaze, level, compact, emoteKind, stillBlink],
  );
  const frame = stillShot ?? live!;

  const view = faceView(size);
  const draw = (fr: Frame, id: string) => drawFace(face, fr, { size, view, reduced, uid: id });
  return (
    <svg
      className={props.className}
      style={{ display: "block", overflow: "hidden", ...props.style }}
      width={faceWidth(size)}
      height={size}
      viewBox={`${f(view.x)} ${f(view.y)} ${f(view.w)} ${f(view.h)}`}
      role="img"
      aria-label={props.label}
      aria-hidden={props.label ? undefined : true}
    >
      {frame.fade ? (
        <>
          <g opacity={1 - frame.fade.a}>{draw(frame.fade.from, `${uid}p`)}</g>
          <g opacity={frame.fade.a}>{draw(frame, uid)}</g>
        </>
      ) : (
        draw(frame, uid)
      )}
    </svg>
  );
}

// ---------- drawing ----------

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
const smooth = (t: number) => {
  const x = clamp(t, 0, 1);
  return x * x * (3 - 2 * x);
};

const HEART_PINK = "#FF4F86";
/** State symbols: fixed colours, so they read the same on every face. */
const CURSOR_LINE = "#E9E9EE";
const ASK_AMBER = "#FFB547";
const ERROR_CORAL = "#FF7A6B";
const OFFLINE_GREY = "#737780";
/** Compact brows appear only where they carry the state. */
const BROW_STATES: ReadonlySet<FaceState> = new Set(["thinking", "error", "approval"]);

interface DrawOpts {
  size: number;
  view: FaceView;
  reduced: boolean;
  uid: string;
}

function drawFace(face: FaceConfig, fr: Frame, o: DrawOpts): ReactElement {
  const { size, view, reduced, uid } = o;
  const P = fr.pose;
  const px = view.px;
  const det = clamp(P.detail, 0, 1);
  const tiny = size < 40;
  const off = clamp(P.off, 0, 1);
  const dim = clamp(P.dim, 0, 1);
  const tone = (c: string) => drain(c, dim);
  const id = (name: string) => `${uid}-${name}`;
  const url = (name: string) => `url(#${id(name)})`;
  // Reduced motion: anything driven by the state clock is frozen at the
  // state's representative moment.
  const vst = reduced ? (HERO_TIME[fr.state] ?? 1.2) : fr.st;

  const R = 11 + face.eyes.size * 13;
  const geom = eyeGeom(face.eyes.shape, R);
  const { hx, hy } = geom;
  const d = Math.max(18 + face.eyes.spacing * 22, hx + 5);
  const flat = face.eyes.iris.size < 0.12 && face.eyes.pupil.size < 0.08;
  /** Pill eyes: solid beans, iris and pupil ignored (see pillEyes). */
  const pill = face.eyes.shape === "pill";

  const sclera = tone(visibleOnBlack(face.eyes.sclera, 0.06));
  const accent = tone(visibleOnBlack(face.palette.accent, 0.14));
  const ink = tone(visibleOnBlack(face.brows.color, 0.16));
  const mouthC = tone(visibleOnBlack(face.mouth.color, 0.14));
  const accC = tone(visibleOnBlack(face.accessory_color, 0.1));
  const lidC = tone(face.eyes.lids.color);
  const irisC = tone(face.eyes.iris.color);
  const pupilC = tone(face.eyes.pupil.color);
  const cheekC = tone(face.cheeks.color);
  const glowC = flat || pill ? sclera : accent;
  const lidVisible = luminance(lidC) > 0.025;
  const thick = det < 0.5 ? 1.3 : 1;

  const glow = clamp(face.eyes.glow, 0, 1) * (1 - off);
  // A face set to no blush never blushes; the states only add to faces that
  // do. Small and faint blush is dropped at closed-island size.
  const blushRaw = face.cheeks.blush > 0 ? clamp(face.cheeks.blush + P.blush, 0, 1) : 0;
  const blush = det < 0.5 && blushRaw < 0.35 ? 0 : blushRaw;

  const gx = clamp(P.gx, -1.2, 1.2);
  const gy = clamp(P.gy, -1.2, 1.2);
  // Whole-eye shift with the gaze; flat eyes keep a steady outline and move
  // a light core inside instead. Pill eyes have nothing inside to move, so
  // the whole eye travels much further.
  const shiftX = gx * (pill ? hx * 0.62 : R * (flat ? 0.05 : 0.08));
  const shiftY = gy * (pill ? hy * 0.36 : R * (flat ? 0.04 : 0.06));
  /** Pill eyes: head-turn roll with the horizontal gaze (degrees), and the
   * springy settle after a blink. */
  const roll = pill ? gx * 5 : 0;
  const rebound = pill && !reduced ? (fr.rebound ?? 0) : 0;

  const headK = det < 0.5 ? 0.5 : 1;
  const nod = fr.state === "accepted" && !reduced ? nodCurve(vst) * (det < 0.5 ? 2 : 3) * px : 0;
  const head = `translate(${f(P.hx * headK)} ${f(P.hy * headK + nod)}) rotate(${f(P.hr * (det < 0.5 ? 0.45 : 1))} 0 ${f(R)})`;
  // Antenna and halo trail the head on a loose spring.
  const wobX = clamp((P.ax - P.hx) * 1.4 - P.hr * 0.25, -R, R);
  const wobY = clamp((P.ay - P.hy) * 1.2, -R, R);

  const eyeData = ([-1, 1] as const).map((s) => {
    const left = s < 0;
    const sc = left ? P.sL : P.sR;
    const lift = (left ? P.eyL : P.eyR) * R;
    const blink = left ? fr.blinkL : fr.blinkR;
    let u = clamp(left ? P.uL : P.uR, 0, 1);
    let l = clamp(left ? P.lL : P.lR, 0, 1);
    u += Math.max(0, 1 - u - l) * blink;
    if (u + l > 1) {
      const sum = u + l;
      u /= sum;
      l /= sum;
    }
    const aperture = Math.max(0, 1 - u - l);
    const tilt = (left ? P.tL : P.tR) * hy * 0.55;
    const yU = -hy + 2 * hy * u;
    const yL = hy - 2 * hy * l;
    const W = hx * 1.4;
    const cU = hy * 0.45;
    const yUin = yU + cU + tilt;
    const yUout = yU + cU - tilt;
    const ctrlU = 2 * yU - (yUin + yUout) / 2;
    // Lower lid: at or below 0 a gentle curve that hugs the eye; above 0
    // an ellipse rising from below, so smiling eyes become a clean crescent.
    const lc = clamp(P.lowCurve, -1, 1);
    let lower: string;
    if (lc <= 0) {
      const yLc = yL + lerp(-0.3, 0, lc + 1) * hy;
      const ctrlL = 2 * yL - yLc;
      lower = `M${f(-W)},${f(hy * 3)}L${f(W)},${f(hy * 3)}L${f(W)},${f(yLc)}Q0,${f(ctrlL)} ${f(-W)},${f(yLc)}Z`;
    } else {
      const k = smooth(lc);
      const rx = hx * lerp(7, 1.65, k);
      const ry = hy * lerp(7, 1.02, k);
      const cy = yL + ry;
      lower = `M${f(-rx)},${f(cy)}A${f(rx)},${f(ry)} 0 1 1 ${f(rx)},${f(cy)}L${f(rx)},${f(hy * 3 + ry)}L${f(-rx)},${f(hy * 3 + ry)}Z`;
    }
    const edgeY = (x: number) => {
      const t = (x + W) / (2 * W);
      return (1 - t) * (1 - t) * yUin + 2 * t * (1 - t) * ctrlU + t * t * yUout;
    };
    const ym = (yU + yL) / 2;
    const lineOp = clamp((0.16 - aperture) / 0.13, 0, 1);
    const contentOp = smooth(aperture / 0.05);
    const k = clamp(P.closedCurve, -1, 1) * hy * 0.24;
    const hw = Math.max(hx * 0.62, hx * Math.sqrt(Math.max(0, 1 - (ym / hy) ** 2))) * 0.95;
    const lineY = (x: number) => {
      const t = (x + hw) / (2 * hw);
      const c = ym - 3 * k;
      const e = ym + k;
      return (1 - t) * (1 - t) * e + 2 * t * (1 - t) * c + t * t * e;
    };
    // Smiling eyes (lower lid bowed up and high) show only the white arc.
    const irisOp = 1 - (lc > 0 ? smooth(lc) * smooth((l - 0.28) / 0.26) : 0);
    return {
      s,
      sc,
      lift,
      irisOp,
      u,
      l,
      blink,
      aperture,
      upper: `M${f(-W)},${f(-hy * 3)}L${f(W)},${f(-hy * 3)}L${f(W)},${f(yUout)}Q0,${f(ctrlU)} ${f(-W)},${f(yUin)}Z`,
      lower,
      edge: `M${f(-W)},${f(yUin)}Q0,${f(ctrlU)} ${f(W)},${f(yUout)}`,
      line: `M${f(-hw)},${f(ym + k)}Q0,${f(ym - 3 * k)} ${f(hw)},${f(ym + k)}`,
      lineOp,
      contentOp,
      edgeY,
      lineY,
      hw,
    };
  });

  const rot = (s: number) => -s * face.eyes.tilt * 16;
  const local = (s: number) => `rotate(${f(rot(s))}) scale(${s} 1)`;

  // ---------- eye parts ----------

  const irisR = flat ? 0 : face.eyes.iris.size * ((hx + hy) / 2) * 0.98;
  const pupilBase = face.eyes.pupil.size * (irisR > 0 ? irisR : Math.min(hx, hy) * 0.62);
  const pupilR = Math.min(pupilBase * clamp(P.pupil, 0.3, 1.8), irisR > 0 ? irisR * 0.94 : Math.min(hx, hy) * 0.8);
  const mover = irisR > 0 ? irisR : pupilR;
  const mx = flat ? 0 : Math.max(hx - mover * 0.7, hx * 0.3);
  const my = flat ? 0 : Math.max(hy - mover * 0.7, hy * 0.22);
  const ix = gx * mx;
  const iy = gy * my;

  const pupil = () => {
    const r = pupilR;
    if (r < 0.2) return null;
    const op = 1 - clamp(P.heart, 0, 1);
    switch (face.eyes.pupil.shape) {
      case "round":
        return <circle r={f(r)} fill={pupilC} opacity={op} />;
      case "slit":
        return <ellipse rx={f(r * 0.3 * (0.55 + 0.45 * P.pupil))} ry={f(Math.min((irisR || hy) * 0.92, r * 1.85))} fill={pupilC} opacity={op} />;
      case "star":
        return <path d={softStar(0, r * 0.05, r * 1.32, r * 0.6, 5, 0.22)} fill={pupilC} opacity={op} />;
      case "heart":
        return <path d={heart(0, 0, r * 1.12)} fill={pupilC} opacity={op} />;
      case "ring":
        return (
          <g opacity={op}>
            <circle r={f(r * 0.84)} fill="none" stroke={pupilC} strokeWidth={f(r * 0.36)} />
            {det > 0.5 ? <circle r={f(r * 0.18)} fill={pupilC} /> : null}
          </g>
        );
    }
  };

  const heartOverlay = () => {
    const h = clamp(P.heart, 0, 1.4);
    if (h < 0.02) return null;
    const size = (irisR > 0 ? irisR * 1.18 : Math.max(hx, hy) * 0.9) * h;
    const pink = tone(HEART_PINK);
    return (
      <g transform={`translate(${f(ix)} ${f(iy)})`}>
        <path d={heart(0, 0, size)} fill={pink} />
        <ellipse cx={f(-size * 0.42)} cy={f(-size * 0.32)} rx={f(size * 0.2)} ry={f(size * 0.13)} fill="#FFFFFF" opacity={0.85} transform={`rotate(-35 ${f(-size * 0.42)} ${f(-size * 0.32)})`} />
      </g>
    );
  };

  /** Flat eyes: a light core that drifts up to 2 px with the gaze. */
  const core = () => {
    if (!flat) return null;
    const cx = gx * Math.min(2 * px, hx * 0.3);
    const cy = gy * Math.min(2 * px, hy * 0.3);
    return <ellipse cx={f(cx)} cy={f(cy)} rx={f(hx * 0.5)} ry={f(hy * 0.46)} fill={url("core")} opacity={0.7 + 0.3 * face.eyes.shine} />;
  };

  const shine = () => {
    const amt = face.eyes.shine;
    if (amt < 0.02 || flat) return null;
    const sr = irisR > 0 ? irisR : Math.max(pupilR, Math.min(hx, hy) * 0.5);
    const cx = ix - sr * 0.36 - gx * sr * 0.1;
    const cy = iy - sr * 0.4 - gy * sr * 0.08;
    const minR = det < 0.5 ? 0.6 * px : 0.6;
    return (
      <g opacity={1 - dim * 0.7}>
        <ellipse cx={f(cx)} cy={f(cy)} rx={f(Math.max(minR, sr * 0.3))} ry={f(Math.max(minR * 0.8, sr * 0.23))} fill="#FFFFFF" opacity={amt * 0.95} transform={`rotate(-30 ${f(cx)} ${f(cy)})`} />
        {det > 0.5 ? <circle cx={f(ix + sr * 0.32)} cy={f(iy + sr * 0.3)} r={f(sr * 0.11)} fill="#FFFFFF" opacity={amt * 0.6 * det} /> : null}
      </g>
    );
  };

  const lashes = (e: (typeof eyeData)[number]) => {
    if (face.eyes.lashes === "none") return null;
    const full = face.eyes.lashes === "full";
    // Closed island below 40 px: one short outer flick, 1 px wide.
    const spec: [number, number, number][] = tiny
      ? [[0.9, 55, (2.6 * px) / R]]
      : full
        ? [
            [0.4, 18, 0.24],
            [0.66, 40, 0.3],
            [0.9, 62, 0.34],
          ]
        : [
            [0.58, 32, 0.22],
            [0.88, 56, 0.27],
          ];
    const down = P.closedCurve < 0 ? e.lineOp : 0;
    const w = tiny ? 1 * px : R * (full ? 0.085 : 0.07) * thick;
    const parts = spec.map(([fx, deg, len], i) => {
      const x = fx * hx * 0.94;
      const top = geom.top(x);
      const open = Number.isNaN(top) ? e.edgeY(x) : Math.max(e.edgeY(x), top);
      const y = lerp(open, e.lineY(Math.min(x, e.hw)), e.lineOp);
      const a = (deg * Math.PI) / 180;
      const L = R * len * (1 - 0.45 * smooth((e.u - 0.25) / 0.35) * (1 - e.lineOp));
      const dx = Math.sin(a) * L;
      const dy = -Math.cos(a) * L * (1 - 2 * down);
      return <path key={i} d={`M${f(x)},${f(y)}Q${f(x + dx * 0.35)},${f(y + dy * 0.75)} ${f(x + dx)},${f(y + dy)}`} />;
    });
    let wing = null;
    if (full && !tiny) {
      const [cx, cy] = geom.corner;
      const x0 = cx - R * 0.3;
      const top = geom.top(x0);
      const y0 = lerp(Number.isNaN(top) ? e.edgeY(x0) : Math.max(e.edgeY(x0), top), e.lineY(Math.min(x0, e.hw)), e.lineOp);
      wing = (
        <path
          d={`M${f(x0)},${f(y0)}Q${f(cx + R * 0.12)},${f(cy - R * 0.06)} ${f(cx + R * 0.42)},${f(cy - R * 0.3)}`}
          strokeWidth={f(w * 1.35)}
          opacity={1 - e.lineOp}
        />
      );
    }
    return (
      <g fill="none" stroke={ink} strokeWidth={f(w)} strokeLinecap="round">
        {parts}
        {wing}
      </g>
    );
  };

  const eye = (e: (typeof eyeData)[number]) => {
    const s = e.s;
    const ex = s * d + shiftX;
    const sx = e.sc * (1 + e.blink * 0.05);
    const sy = e.sc * (1 - e.blink * 0.04);
    const clipId = id(s < 0 ? "cl" : "cr");
    const maskId = id(s < 0 ? "ml" : "mr");
    const lineW = Math.max(1.8, R * 0.17) * thick;
    const glowR = lerp(1.3, 1.6, det);
    return (
      <g key={s} opacity={1 - off}>
        <g transform={`translate(${f(ex)} ${f(shiftY + e.lift)}) scale(${f(sx)} ${f(sy)})`}>
          <clipPath id={clipId}>
            <path d={geom.path} transform={local(s)} />
          </clipPath>
          <mask id={maskId}>
            <g transform={local(s)}>
              <path d={geom.path} fill="#FFFFFF" />
              <path d={e.lower} fill="#000000" />
              <path d={e.upper} fill="#000000" />
            </g>
          </mask>
          {glow > 0.01 ? <ellipse rx={f(hx * glowR)} ry={f(hy * glowR)} fill={url("glow")} opacity={glow * smooth(e.aperture / 0.6)} /> : null}
          {lidVisible ? (
            <g clipPath={`url(#${clipId})`} opacity={e.contentOp}>
              <g transform={local(s)}>
                <path d={e.upper} fill={lidC} />
                <path d={e.edge} fill="none" stroke={darken(lidC, 0.4)} strokeWidth={f(R * 0.07)} />
              </g>
            </g>
          ) : null}
          <g mask={`url(#${maskId})`} opacity={e.contentOp}>
            <g transform={local(s)}>
              <path d={geom.path} fill={flat ? url("flat") : sclera} />
              {!flat && luminance(sclera) > 0.3 ? <path d={e.upper} fill="#000000" opacity={0.09} transform={`translate(0 ${f(hy * 0.13)})`} /> : null}
            </g>
            {core()}
            {irisR > 0 || pupilR > 0 ? (
              <g opacity={e.irisOp} transform={`translate(${f(ix)} ${f(iy)}) scale(${f(1 - 0.16 * Math.abs(gx))} ${f(1 - 0.1 * Math.abs(gy))})`}>
                {irisR > 0 ? (
                  <>
                    <circle r={f(irisR)} fill={det < 0.5 ? irisC : url("iris")} />
                    {det > 0.5 ? <circle r={f(irisR * 0.95)} fill="none" stroke={darken(irisC, 0.4)} strokeWidth={f(irisR * 0.08)} opacity={0.4} /> : null}
                  </>
                ) : null}
                {pupil()}
              </g>
            ) : null}
            {heartOverlay()}
            <g opacity={e.irisOp}>{shine()}</g>
          </g>
          <g transform={local(s)}>
            {e.lineOp > 0.01 ? (
              <path d={e.line} fill="none" stroke={flat ? sclera : lighten(sclera, 0.1)} strokeWidth={f(lineW)} strokeLinecap="round" opacity={e.lineOp} />
            ) : null}
            {lashes(e)}
          </g>
        </g>
      </g>
    );
  };

  // ---------- pill eyes ----------

  /** Pill eyes: two solid beans in the sclera colour. The gaze moves and
   * turns the whole eye, the far eye a little narrower and the pair rolled
   * like a turning head (the eye on the side looked at sits lower). The lids
   * squash the bean from the top (u) and the bottom (l); closed it is a
   * capsule bar, anchored low like the reference blink, that closedCurve
   * bends (+ happy arch, - relaxed curve) and a smile under the eyes (high
   * l with lowCurve up) turns into a happy arch too. tL / tR turn each eye
   * (+ inner side lower), `stretch` makes both taller or flatter. */
  const pillEyes = () => {
    const rr = hx * PILL_EYE.r;
    const bar = Math.max(hy * 0.24, 2.4 * px);
    const lc = clamp(P.lowCurve, -1, 1);
    const vert = -gy * 0.1;
    const pink = tone(HEART_PINK);
    return eyeData.map((e) => {
      const s = e.s;
      const left = s < 0;
      const top = -hy + 2 * hy * e.u;
      const bot = hy - 2 * hy * e.l;
      const open = bot - top;
      const closed = smooth(1 - (open - bar) / (bar * 2));
      const happy = lc > 0 ? smooth(lc) * smooth((e.l - 0.3) / 0.3) : 0;
      const h = lerp(Math.max(bar, open), bar * 1.35, happy);
      const cy = lerp(open >= bar ? (top + bot) / 2 : bot - bar / 2, -hy * 0.18, happy);
      const far = Math.max(0, s * gx) * 0.08;
      const H = h * Math.max(0.3, 1 + P.stretch + vert - rebound * 0.1);
      const W = 2 * hx * (1 + 0.1 * closed + 0.14 * happy - 0.35 * P.stretch + rebound * 0.05 - far);
      const curve = clamp(P.closedCurve, -1, 1) * closed;
      // Only a capsule bends; a taller bean first flattens into one.
      const bend = lerp(curve, 1, happy) * smooth((2 * rr - H) / (rr * 0.8));
      const ang = roll - s * (face.eyes.tilt * 16 + (left ? P.tL : P.tR) * 14) + e.blink * 4 * (gx < -0.05 ? -1 : 1);
      const ex = s * d + shiftX;
      const ey = shiftY + e.lift + s * gx * hy * 0.2 + rebound * hy * 0.16;
      const hk = clamp(P.heart, 0, 1.4);
      const bean = 1 - 0.65 * Math.min(1, hk);
      let body: ReactElement;
      if (Math.abs(bend) > 0.01 && H < 2 * rr) {
        const half = Math.max(0, W / 2 - H / 2);
        const depth = bend * W * 0.3;
        const y1 = cy + depth / 2;
        body = <path d={`M${f(-half)},${f(y1)}Q0,${f(y1 - 2 * depth)} ${f(half)},${f(y1)}`} fill="none" stroke={sclera} strokeWidth={f(H)} strokeLinecap="round" />;
      } else {
        body = <path d={pillPath(W, H, rr, cy)} fill={sclera} />;
      }
      const shineAmt = luminance(sclera) < 0.8 ? face.eyes.shine * (1 - closed) : 0;
      return (
        <g key={s} opacity={1 - off} transform={`translate(${f(ex)} ${f(ey)}) rotate(${f(ang)}) scale(${f(s * e.sc)} ${f(e.sc)})`}>
          {glow > 0.01 ? <ellipse cy={f(cy)} rx={f(W * 0.78)} ry={f(H * 0.74)} fill={url("glow")} opacity={glow * (1 - 0.6 * closed)} /> : null}
          <g opacity={hk > 0.02 ? 1 - Math.min(1, hk) * 0.85 : 1} transform={bean < 1 ? `translate(0 ${f(cy)}) scale(${f(bean)}) translate(0 ${f(-cy)})` : undefined}>
            {body}
            {shineAmt > 0.02 ? <ellipse cx={f(-W * 0.18)} cy={f(cy - H * 0.27)} rx={f(W * 0.14)} ry={f(H * 0.09)} fill={lighten(sclera, 0.75)} opacity={shineAmt * 0.8} /> : null}
          </g>
          {hk > 0.02 ? <path d={heart(0, cy, Math.max(hx, hy) * 0.78 * hk)} fill={pink} /> : null}
          {pillLashes(W, H, cy, closed)}
        </g>
      );
    });
  };

  /** Lashes on a pill eye: short flicks off the top outer corner. */
  const pillLashes = (W: number, H: number, cy: number, closed: number) => {
    if (face.eyes.lashes === "none" || closed > 0.95) return null;
    const full = face.eyes.lashes === "full";
    const spec: [number, number, number][] = tiny
      ? [[0.82, 50, (2.6 * px) / R]]
      : full
        ? [
            [0.3, 14, 0.22],
            [0.62, 34, 0.28],
            [0.92, 58, 0.32],
          ]
        : [
            [0.5, 26, 0.2],
            [0.86, 50, 0.25],
          ];
    const a = W / 2;
    const r = Math.min(hx * PILL_EYE.r, H / 2, a);
    const w = tiny ? 1 * px : R * (full ? 0.085 : 0.07) * thick;
    const topAt = (x: number) => {
      const dx = Math.max(0, x - (a - r));
      return cy - H / 2 + (r - Math.sqrt(Math.max(0, r * r - dx * dx)));
    };
    return (
      <g fill="none" stroke={ink} strokeWidth={f(w)} strokeLinecap="round" opacity={1 - closed}>
        {spec.map(([fx, deg, len], i) => {
          const x = fx * a;
          const y = topAt(x);
          const ang = (deg * Math.PI) / 180;
          const L = R * len;
          const dx = Math.sin(ang) * L;
          const dy = -Math.cos(ang) * L;
          return <path key={i} d={`M${f(x)},${f(y)}Q${f(x + dx * 0.35)},${f(y + dy * 0.75)} ${f(x + dx)},${f(y + dy)}`} />;
        })}
      </g>
    );
  };

  /** Offline: two still grey capsules, 8 x 2 px, no glow. */
  const capsules = () => {
    if (off < 0.01) return null;
    const w = 8 * px * lerp(1, 1.5, det);
    const h = 2 * px * lerp(1, 1.5, det);
    return (
      <g opacity={off} fill={OFFLINE_GREY}>
        {[-1, 1].map((s) => (
          <rect key={s} x={f(s * d - w / 2)} y={f(hy * 0.1 - h / 2)} width={f(w)} height={f(h)} rx={f(h / 2)} />
        ))}
      </g>
    );
  };

  // ---------- brows, mouth, cheeks ----------

  const brows = () => {
    const style = face.brows.style;
    if (style === "none") return null;
    // Closed island: a short simple brow, only in states it helps tell apart.
    const stateBrow = BROW_STATES.has(fr.state) ? 1 : 0;
    const op = Math.max(det, stateBrow * (1 - off));
    if (op < 0.01) return null;
    const spec = {
      thin: { w: 0.12, arch: 0.35, angle: 0, lift: 0, half: 0.7 },
      thick: { w: 0.26, arch: 0.18, angle: 0, lift: 0, half: 0.66 },
      arched: { w: 0.15, arch: 0.75, angle: 0, lift: 0.12, half: 0.72 },
      flat: { w: 0.19, arch: 0.02, angle: 0, lift: 0, half: 0.68 },
      worried: { w: 0.15, arch: 0.22, angle: 13, lift: 0.05, half: 0.68 },
    }[style];
    const width = lerp(1.4 * px, R * spec.w, det);
    const half = lerp(3.2 * px, R * spec.half, det);
    return (
      <g fill="none" stroke={ink} strokeLinecap="round" strokeWidth={f(width)} opacity={op}>
        {eyeData.map((e) => {
          const s = e.s;
          const left = s < 0;
          const lift = (left ? P.bLl : P.bRl) + spec.lift;
          const angle = (left ? P.bLa : P.bRa) + spec.angle - face.eyes.tilt * 8;
          const lidDown = (left ? P.uL : P.uR) - face.eyes.lids.rest * LID_REST;
          // Closed island: the brow sits close above the eye so it stays inside the shape.
          const gap = lerp(2.4 * px, R * 0.42 + 3, det);
          const liftK = lerp(1.6 * px, R * 0.42, det);
          const cy = shiftY * 0.4 + e.lift - hy * e.sc - gap - lift * liftK + Math.max(0, lidDown) * R * 0.32 * det + e.blink * R * 0.05;
          const cx = s * (d + R * 0.06) + shiftX * 0.4;
          const dy = Math.tan((clamp(angle, -35, 35) * Math.PI) / 180) * half;
          const innerX = cx - s * half;
          const outerX = cx + s * half;
          const innerY = cy - dy;
          const outerY = cy + dy * 0.5;
          const midY = (innerY + outerY) / 2 - spec.arch * R * 0.3 * (half / (R * spec.half));
          const ctrlY = 2 * midY - (innerY + outerY) / 2;
          return <path key={s} d={`M${f(innerX)},${f(innerY)}Q${f(cx)},${f(ctrlY)} ${f(outerX)},${f(outerY)}`} />;
        })}
      </g>
    );
  };

  const mouthY = Math.max(hy, R * 0.9) + R * 0.52 + 2;

  // Pill faces: a thin smile tucked between the eyes, as low as their
  // bottoms. It is part of the look, so it stays on the closed island too,
  // rides along with the eyes and drops a little with each blink.
  const mouthOp = pill ? Math.max(det, 1 - off) : det;
  const mouthRoll = roll * 0.8;
  const mouthDrop = pill ? ((fr.blinkL + fr.blinkR) / 2) * hy * 0.14 + rebound * hy * 0.1 : 0;
  const mouthAt = pill
    ? { x: P.mouthX * R * 0.3 + shiftX * 0.55, y: hy * 0.86 + shiftY * 0.5 + mouthDrop }
    : { x: P.mouthX * R * 0.3, y: mouthY };

  const mouth = () => {
    const m = mouthShape();
    return m && mouthRoll ? <g transform={`rotate(${f(mouthRoll)} ${f(mouthAt.x)} ${f(mouthAt.y)})`}>{m}</g> : m;
  };

  const mouthShape = () => {
    const style = face.mouth.style;
    if (mouthOp < 0.01 || style === "none") return null;
    const sm = clamp(P.smile, -1, 1);
    const op = clamp(P.open, 0, 1.2);
    const x0 = mouthAt.x;
    const y0 = mouthAt.y;
    const lw = pill ? Math.max(1.25 * px, hy * 0.085) : Math.max(1.5, R * 0.12);
    const inside = darken(mouthC, 0.72);

    if (style === "dot") {
      const r = R * 0.11 * (1 + op * 1.7);
      if (op < 0.1) return <circle cx={f(x0)} cy={f(y0)} r={f(r)} fill={mouthC} opacity={mouthOp} />;
      return <ellipse cx={f(x0)} cy={f(y0 + r * 0.2)} rx={f(r * 0.85)} ry={f(r * 1.1)} fill={inside} stroke={mouthC} strokeWidth={f(lw * 0.9)} opacity={mouthOp} />;
    }

    // A cat mouth stays a cat mouth while content; unhappy, it flattens.
    if (style === "cat" && sm >= -0.05) {
      const w = R * 0.42;
      const depth = R * (0.17 + Math.max(0, sm) * 0.08);
      const y = y0 - R * 0.06;
      const lip = `M${f(x0 - w)},${f(y - depth * 0.35)}Q${f(x0 - w / 2)},${f(y + depth * 1.2)} ${f(x0)},${f(y)}Q${f(x0 + w / 2)},${f(y + depth * 1.2)} ${f(x0 + w)},${f(y - depth * 0.35)}`;
      return (
        <g opacity={mouthOp}>
          {op > 0.05 ? (
            <ellipse cx={f(x0)} cy={f(y + depth * 0.5 + op * R * 0.22)} rx={f(w * 0.42)} ry={f(op * R * 0.34 + R * 0.04)} fill={inside} stroke={mouthC} strokeWidth={f(lw * 0.8)} />
          ) : null}
          <path d={lip} fill="none" stroke={mouthC} strokeWidth={f(lw)} strokeLinecap="round" strokeLinejoin="round" />
        </g>
      );
    }

    const w = pill ? hy * (style === "line" ? 0.55 : 0.7) : R * (style === "line" || style === "cat" ? 0.4 : 0.5);
    const sag = pill ? hy * 0.17 : R * 0.15;
    const yc = y0 - sm * sag;
    const mid = y0 + sm * sag;
    const tiltY = P.mouthX * R * 0.12;
    const lx = x0 - w;
    const rx = x0 + w;
    const ly = yc + tiltY;
    const ry = yc - tiltY;
    const cU = 2 * mid - (ly + ry) / 2;
    if (op < 0.04) {
      return <path d={`M${f(lx)},${f(ly)}Q${f(x0)},${f(cU)} ${f(rx)},${f(ry)}`} fill="none" stroke={mouthC} strokeWidth={f(lw)} strokeLinecap="round" opacity={mouthOp} />;
    }
    const lowMid = mid + op * R * 0.8;
    const cL = 2 * lowMid - (ly + ry) / 2;
    const shape = `M${f(lx)},${f(ly)}Q${f(x0)},${f(cU)} ${f(rx)},${f(ry)}Q${f(x0)},${f(cL)} ${f(lx)},${f(ly)}Z`;
    return (
      <g opacity={mouthOp}>
        <clipPath id={id("m")}>
          <path d={shape} />
        </clipPath>
        <path d={shape} fill={inside} />
        <ellipse cx={f(x0)} cy={f(lowMid)} rx={f(w * 0.62)} ry={f(op * R * 0.36)} fill={tone(mix(mouthC, "#FF6F91", 0.5))} clipPath={url("m")} />
        <path d={shape} fill="none" stroke={mouthC} strokeWidth={f(lw * 0.9)} strokeLinejoin="round" />
      </g>
    );
  };

  const cheeks = () => {
    if (blush < 0.01) return null;
    const cy = hy * 0.72 + R * lerp(0.16, 0.3, det);
    return (
      <g opacity={Math.min(1, blush * 1.25) * (1 - off)}>
        {[-1, 1].map((s) => (
          <ellipse key={s} cx={f(s * (d + hx * 0.62))} cy={f(cy)} rx={f(R * 0.5)} ry={f(R * 0.3)} fill={url("cheek")} />
        ))}
      </g>
    );
  };

  // ---------- accessories ----------

  const eyeTop = -hy;
  const acc = face.accessory;
  const accLw = Math.max(1.4, R * 0.13) * thick;

  /** Glasses ride the nose: pushed up, slid down or askew by the pose, but
   * on a closed island never more than 3 px, and never closer than 2 px to
   * the edge of the black shape. */
  const glassesAt = (H: number, top: number, bottom: number) => {
    const want = P.gl * H;
    const maxUp = det < 0.5 ? 3 * px : H * 0.9;
    const roomUp = top - (view.y + 2 * px) + P.hy * headK;
    const roomDown = view.y + view.h - 2 * px - bottom - P.hy * headK;
    const lift = clamp(want, -Math.max(0, roomDown), Math.max(0, Math.min(maxUp, roomUp)));
    return `translate(${f(shiftX * 0.22)} ${f(shiftY * 0.22 - lift)}) rotate(${f(P.glr)} 0 ${f(-H * 0.3)})`;
  };

  const glasses = (): ReactElement | null => {
    if (acc === "glasses" || acc === "round_glasses") {
      const round = acc === "round_glasses";
      const fw = hx + R * 0.34;
      const fh = hy + R * 0.3;
      const rr = Math.max(hx, hy) + R * 0.32;
      const inner = d - (round ? rr : fw);
      const half = round ? rr : fh;
      return (
        <g fill="none" stroke={accC} strokeWidth={f(accLw)} strokeLinecap="round" transform={glassesAt(fh * 0.6, -half, half)}>
          {[-1, 1].map((s) =>
            round ? (
              <circle key={s} cx={f(s * d)} cy={0} r={f(rr)} fill="#FFFFFF" fillOpacity={0.04} />
            ) : (
              <rect key={s} x={f(s * d - fw)} y={f(-fh)} width={f(2 * fw)} height={f(2 * fh)} rx={f(R * 0.5)} fill="#FFFFFF" fillOpacity={0.04} />
            ),
          )}
          <path d={`M${f(-inner)},${f(-hy * 0.25)}Q0,${f(-hy * 0.5)} ${f(inner)},${f(-hy * 0.25)}`} strokeWidth={f(accLw * 0.85)} />
          {det > 0.01
            ? [-1, 1].map((s) => {
                const x = s * (d + (round ? rr : fw));
                return <path key={`t${s}`} d={`M${f(x)},${f(-hy * 0.35)}L${f(x + s * R * 0.55)},${f(-hy * 0.55)}`} opacity={det} />;
              })
            : null}
        </g>
      );
    }
    if (acc === "sunglasses") {
      const W = hx + R * 0.38;
      const H = hy + R * 0.28;
      const lens = `M${f(-0.95 * W)},${f(-0.72 * H)}C${f(-0.95 * W)},${f(-1.02 * H)} ${f(1.05 * W)},${f(-1.08 * H)} ${f(1.06 * W)},${f(-0.62 * H)}C${f(1.08 * W)},${f(0.45 * H)} ${f(0.6 * W)},${f(1.0 * H)} 0,${f(1.0 * H)}C${f(-0.62 * W)},${f(1.0 * H)} ${f(-1.0 * W)},${f(0.4 * H)} ${f(-0.95 * W)},${f(-0.72 * H)}Z`;
      const rim = lighten(accC, 0.3);
      const inner = d - 0.95 * W;
      // Closed island: thin lenses so the eyes (and the state) show through.
      const lensOp = lerp(0.4, 0.74, det);
      return (
        <g transform={glassesAt(H * 1.6, -1.0 * H, 1.0 * H)}>
          {[-1, 1].map((s) => (
            <g key={s} transform={`translate(${f(s * d)} 0) scale(${s} 1)`}>
              <path d={lens} fill={url("lens")} opacity={lensOp} />
              <path d={lens} fill="none" stroke={rim} strokeWidth={f(R * 0.08 * thick)} strokeOpacity={0.9} />
              {det > 0.01 ? (
                <path
                  d={`M${f(-0.62 * W)},${f(-0.35 * H)}L${f(-0.28 * W)},${f(-0.86 * H)}L${f(-0.08 * W)},${f(-0.86 * H)}L${f(-0.44 * W)},${f(-0.3 * H)}Z`}
                  fill="#FFFFFF"
                  opacity={0.2 * det}
                />
              ) : null}
            </g>
          ))}
          <path d={`M${f(-inner)},${f(-0.72 * H)}Q0,${f(-0.98 * H)} ${f(inner)},${f(-0.72 * H)}`} fill="none" stroke={rim} strokeWidth={f(R * 0.12 * thick)} strokeLinecap="round" />
          {det > 0.01
            ? [-1, 1].map((s) => (
                <path key={`t${s}`} d={`M${f(s * (d + 1.05 * W))},${f(-0.6 * H)}L${f(s * (d + 1.05 * W + R * 0.5))},${f(-0.72 * H)}`} stroke={rim} strokeWidth={f(R * 0.1)} strokeLinecap="round" opacity={det} />
              ))
            : null}
        </g>
      );
    }
    return null;
  };

  const topInner = (): ReactElement | null => {
    switch (acc) {
      case "cat_ears": {
        const inner = mix(cheekC, "#FFFFFF", 0.25);
        return (
          <g>
            {[-1, 1].map((s) => (
              <g key={s} transform={`translate(${f(s * (d + hx * 0.2))} ${f(eyeTop - R * 0.5)}) rotate(${f(s * (22 + P.ear * 0.6))}) scale(${s} 1)`}>
                <path
                  d={`M${f(-R * 0.78)},${f(R * 0.05)}Q${f(-R * 0.55)},${f(-R * 0.7)} ${f(-R * 0.05)},${f(-R * 1.22)}Q${f(R * 0.12)},${f(-R * 1.36)} ${f(R * 0.2)},${f(-R * 1.15)}Q${f(R * 0.55)},${f(-R * 0.55)} ${f(R * 0.72)},${f(R * 0.05)}Z`}
                  fill={accC}
                />
                <path d={`M${f(-R * 0.42)},${f(-R * 0.12)}Q${f(-R * 0.28)},${f(-R * 0.6)} ${f(0)},${f(-R * 0.92)}Q${f(R * 0.25)},${f(-R * 0.5)} ${f(R * 0.38)},${f(-R * 0.12)}Z`} fill={inner} />
              </g>
            ))}
          </g>
        );
      }
      case "bow": {
        const fold = darken(accC, 0.28);
        const wing = `M0,0C${f(-R * 0.3)},${f(-R * 0.6)} ${f(-R * 1.05)},${f(-R * 0.72)} ${f(-R * 0.98)},${f(-R * 0.02)}C${f(-R * 0.95)},${f(R * 0.6)} ${f(-R * 0.32)},${f(R * 0.42)} 0,0Z`;
        return (
          <g transform={`translate(${f(d + hx + R * 0.15)} ${f(eyeTop - R * 1.05)}) rotate(${f(24 + P.ear * 0.5)}) scale(0.72)`}>
            <path d={`M${f(-R * 0.08)},${f(R * 0.1)}L${f(-R * 0.42)},${f(R * 0.82)}L${f(-R * 0.14)},${f(R * 0.72)}Z`} fill={fold} />
            <path d={`M${f(R * 0.08)},${f(R * 0.1)}L${f(R * 0.4)},${f(R * 0.8)}L${f(R * 0.14)},${f(R * 0.7)}Z`} fill={fold} />
            <path d={wing} fill={accC} />
            <path d={wing} fill={accC} transform="scale(-1 1)" />
            <path d={`M${f(-R * 0.25)},${f(-R * 0.12)}Q${f(-R * 0.6)},${f(-R * 0.1)} ${f(-R * 0.75)},${f(R * 0.05)}`} fill="none" stroke={fold} strokeWidth={f(R * 0.06)} strokeLinecap="round" />
            <path d={`M${f(R * 0.25)},${f(-R * 0.12)}Q${f(R * 0.6)},${f(-R * 0.1)} ${f(R * 0.75)},${f(R * 0.05)}`} fill="none" stroke={fold} strokeWidth={f(R * 0.06)} strokeLinecap="round" />
            <ellipse rx={f(R * 0.24)} ry={f(R * 0.28)} fill={lighten(accC, 0.12)} />
          </g>
        );
      }
      case "halo": {
        const bob = reduced ? 0 : Math.sin(fr.t * 2.1) * 0.7;
        const cy = eyeTop - R * 1.55 + wobY * 0.6 + bob;
        const rx = Math.min(d * 0.82 + R * 0.25, 44);
        return (
          <g fill="none" stroke={accC} transform={`translate(${f(wobX * 0.5)} 0)`}>
            <ellipse cy={f(cy)} rx={f(rx)} ry={f(R * 0.27)} strokeWidth={f(R * 0.3)} opacity={0.5} filter={url("blur")} />
            <ellipse cy={f(cy)} rx={f(rx)} ry={f(R * 0.27)} strokeWidth={f(R * 0.15)} />
            <path d={`M${f(-rx * 0.7)},${f(cy + R * 0.18)}Q0,${f(cy + R * 0.42)} ${f(rx * 0.7)},${f(cy + R * 0.18)}`} stroke={lighten(accC, 0.6)} strokeWidth={f(R * 0.05)} strokeLinecap="round" />
          </g>
        );
      }
      case "horns": {
        const hi = lighten(accC, 0.35);
        return (
          <g>
            {[-1, 1].map((s) => (
              <g key={s} transform={`translate(${f(s * (d + hx * 0.15))} ${f(eyeTop - R * 0.95)}) rotate(${f(s * 18)}) scale(${s} 1)`}>
                <path
                  d={`M${f(-R * 0.3)},${f(R * 0.08)}C${f(-R * 0.26)},${f(-R * 0.55)} ${f(R * 0.02)},${f(-R * 1.0)} ${f(R * 0.52)},${f(-R * 1.22)}C${f(R * 0.3)},${f(-R * 0.78)} ${f(R * 0.3)},${f(-R * 0.34)} ${f(R * 0.3)},${f(R * 0.08)}Z`}
                  fill={accC}
                />
                <path d={`M${f(-R * 0.14)},${f(-R * 0.1)}C${f(-R * 0.08)},${f(-R * 0.5)} ${f(R * 0.1)},${f(-R * 0.82)} ${f(R * 0.36)},${f(-R * 1.02)}`} fill="none" stroke={hi} strokeWidth={f(R * 0.07)} strokeLinecap="round" opacity={0.7} />
              </g>
            ))}
          </g>
        );
      }
      case "crown": {
        const w = R * 0.85;
        const h = R * 0.95;
        const gem = lighten(accC, 0.65);
        return (
          <g transform={`translate(${f(d * 0.22)} ${f(eyeTop - R * 0.95)}) rotate(-9)`}>
            <path
              d={`M${f(-w)},0L${f(-w)},${f(-0.62 * h)}L${f(-0.48 * w)},${f(-0.26 * h)}L0,${f(-h)}L${f(0.48 * w)},${f(-0.26 * h)}L${f(w)},${f(-0.62 * h)}L${f(w)},0Z`}
              fill={accC}
              stroke={accC}
              strokeWidth={f(R * 0.12)}
              strokeLinejoin="round"
            />
            <rect x={f(-w)} y={f(-R * 0.22)} width={f(2 * w)} height={f(R * 0.22)} fill={darken(accC, 0.18)} />
            <circle cx={f(-w)} cy={f(-0.66 * h)} r={f(R * 0.11)} fill={gem} />
            <circle cx={0} cy={f(-h - R * 0.04)} r={f(R * 0.13)} fill={gem} />
            <circle cx={f(w)} cy={f(-0.66 * h)} r={f(R * 0.11)} fill={gem} />
          </g>
        );
      }
      case "antenna": {
        const baseY = eyeTop - R * 0.6;
        const tipX = wobX * 1.5;
        const tipY = eyeTop - R * 1.85 + wobY;
        const busy = fr.state === "thinking" || fr.state === "working";
        const pulse = busy && !reduced ? 0.5 + 0.5 * Math.sin(fr.t * 6) : 0.4;
        return (
          <g>
            <path d={`M0,${f(baseY)}Q${f(tipX * 0.3)},${f((baseY + tipY) / 2)} ${f(tipX)},${f(tipY)}`} fill="none" stroke={darken(accC, 0.3)} strokeWidth={f(R * 0.1)} strokeLinecap="round" />
            <ellipse cy={f(baseY)} rx={f(R * 0.3)} ry={f(R * 0.1)} fill={darken(accC, 0.35)} />
            <circle cx={f(tipX)} cy={f(tipY)} r={f(R * 0.75)} fill={url("acc-glow")} opacity={0.3 + pulse * 0.5} />
            <circle cx={f(tipX)} cy={f(tipY)} r={f(R * 0.3)} fill={accC} />
            <circle cx={f(tipX - R * 0.09)} cy={f(tipY - R * 0.1)} r={f(R * 0.09)} fill="#FFFFFF" opacity={0.7} />
          </g>
        );
      }
      default:
        return null;
    }
  };

  const top = (): ReactElement | null => {
    if (det < 0.01) return null;
    const inner = topInner();
    return inner ? <g opacity={det}>{inner}</g> : null;
  };


  const headphones = (part: "band" | "cups"): ReactElement | null => {
    if (det < 0.01 || acc !== "headphones") return null;
    const cx = d + hx + R * 0.42;
    if (part === "band") {
      const topY = eyeTop - R * 2.45;
      return (
        <path
          d={`M${f(-cx)},${f(-R * 0.6)}C${f(-cx - R * 0.1)},${f(topY)} ${f(cx + R * 0.1)},${f(topY)} ${f(cx)},${f(-R * 0.6)}`}
          fill="none"
          stroke={accC}
          strokeWidth={f(R * 0.2)}
          strokeLinecap="round"
          opacity={det}
        />
      );
    }
    return (
      <g opacity={det}>
        {[-1, 1].map((s) => (
          <g key={s} transform={`translate(${f(s * cx)} ${f(R * 0.12)})`}>
            <rect x={f(-R * 0.36)} y={f(-R * 0.76)} width={f(R * 0.72)} height={f(R * 1.52)} rx={f(R * 0.34)} fill={accC} />
            <rect x={f(s > 0 ? -R * 0.36 : R * 0.12)} y={f(-R * 0.6)} width={f(R * 0.24)} height={f(R * 1.2)} rx={f(R * 0.12)} fill={darken(accC, 0.45)} />
          </g>
        ))}
      </g>
    );
  };

  // ---------- state symbols, beside the right eye ----------

  const symbolX = Math.min(d + hx + 7.5 * px, view.x + view.w - 6 * px);
  const symbolK = lerp(1, 1.35, det);
  const symbolIn = reduced ? 1 : smooth(fr.st / 0.15);

  const symbol = (): ReactElement | null => {
    const at = (inner: ReactElement, dx = 0, dy = 0) => (
      <g transform={`translate(${f(symbolX + dx)} ${f(dy)}) scale(${f(px * symbolK)})`} opacity={symbolIn}>
        {inner}
      </g>
    );
    switch (fr.state) {
      case "driving":
        // A pointer, 7 x 10 px: dark body, pale outline, nudged by the gaze.
        return at(
          <path
            d="M-3.2,-4.9L-3.2,3.7L-1,1.7L0.6,4.9L2,4.3L0.5,1.2L3.4,1.2Z"
            fill="#000000"
            stroke={CURSOR_LINE}
            strokeWidth={1.25}
            strokeLinejoin="round"
          />,
          gx * 1.5 * px,
          gy * 1.5 * px,
        );
      case "approval":
        // "?" 6 x 8 px, amber.
        return at(
          <g fill="none" stroke={ASK_AMBER} strokeWidth={1.6} strokeLinecap="round" strokeLinejoin="round">
            <path d="M-2.4,-1.9C-2.4,-3.5 -1.3,-4.2 0,-4.2C1.4,-4.2 2.5,-3.3 2.5,-2.1C2.5,-0.8 1.4,-0.4 0.5,0.3C0.1,0.6 0,1 0,1.5" />
            <circle cy={3.5} r={0.95} fill={ASK_AMBER} stroke="none" />
          </g>,
        );
      case "error":
        // "!" 6 x 8 px, coral.
        return at(
          <g fill={ERROR_CORAL}>
            <path d="M0,-4.2L0,1.2" stroke={ERROR_CORAL} strokeWidth={2} strokeLinecap="round" />
            <circle cy={3.6} r={1.1} />
          </g>,
        );
      case "listening": {
        // Three bars, 1.5 px wide with 1.5 px gaps, 3 to 10 px tall; never
        // below 45 % opacity while the microphone is open.
        const lv = [fr.level * 0.85, Math.max(fr.level, fr.levelSlow * 1.1), fr.levelSlow * 0.9];
        return at(
          <g fill={accent} opacity={0.45 + 0.55 * clamp(fr.level * 1.6, 0, 1)}>
            {lv.map((v, i) => {
              const h = 3 + 7 * clamp(v, 0, 1);
              return <rect key={i} x={-3.75 + i * 3} y={-h / 2} width={1.5} height={h} rx={0.75} />;
            })}
          </g>,
        );
      }
      default:
        return null;
    }
  };

  // ---------- effects ----------

  const zees = () => {
    if (fr.state !== "sleeping") return null;
    const items: ReactElement[] = [];
    const life = 3.6;
    const every = 1.5;
    const n = Math.floor(fr.st / every);
    for (let j = 0; j < 3; j++) {
      const age = reduced ? (j === 0 ? life * 0.35 : -1) : fr.st - (n - j) * every;
      if (age < 0 || age > life) continue;
      const u = age / life;
      const x = d + hx * 0.55 + u * R * 1.1 + Math.sin(u * 6) * R * 0.12;
      const y = -hy * 0.3 - u * R * lerp(0.9, 2.0, det);
      const s = R * (0.16 + u * 0.18) * lerp(1.2, 1, det);
      const op = Math.min(1, u / 0.15) * (1 - smooth((u - 0.55) / 0.45));
      items.push(
        <path
          key={j}
          d={`M${f(x - s)},${f(y - s)}L${f(x + s)},${f(y - s)}L${f(x - s)},${f(y + s)}L${f(x + s)},${f(y + s)}`}
          fill="none"
          stroke={accent}
          strokeWidth={f(Math.max(1.2, s * 0.38))}
          strokeLinecap="round"
          strokeLinejoin="round"
          opacity={op}
        />,
      );
    }
    return <g>{items}</g>;
  };

  /** Done: one small sparkle by the right eye. */
  const sparkles = () => {
    if (fr.state !== "done") return null;
    const age = vst - 0.1;
    const life = 0.8;
    if (age <= 0 || age >= life) return null;
    const k = Math.sin((Math.PI * age) / life);
    const x = d + hx * 0.85;
    const y = -hy * 0.85;
    const r = 1.8 * px * k * symbolK;
    return <path d={sparkle(x, y, r)} fill={lighten(accent, 0.25)} transform={`rotate(${f(reduced ? 0 : age * 80)} ${f(x)} ${f(y)})`} />;
  };

  const lensTop = darken(accC, 0.85);
  return (
    <>
      <defs>
        <radialGradient id={id("iris")} cx="50%" cy="62%" r="62%">
          <stop offset="0%" stopColor={lighten(irisC, 0.18)} />
          <stop offset="100%" stopColor={darken(irisC, 0.2)} />
        </radialGradient>
        <radialGradient id={id("flat")} cx="50%" cy="45%" r="60%">
          <stop offset="0%" stopColor={sclera} />
          <stop offset="100%" stopColor={darken(sclera, 0.14)} />
        </radialGradient>
        <radialGradient id={id("core")}>
          <stop offset="0%" stopColor={lighten(sclera, 0.88)} stopOpacity={1} />
          <stop offset="55%" stopColor={lighten(sclera, 0.45)} stopOpacity={0.45} />
          <stop offset="100%" stopColor={sclera} stopOpacity={0} />
        </radialGradient>
        <radialGradient id={id("glow")}>
          <stop offset="0%" stopColor={glowC} stopOpacity={0.55} />
          <stop offset="50%" stopColor={glowC} stopOpacity={0.18} />
          <stop offset="100%" stopColor={glowC} stopOpacity={0} />
        </radialGradient>
        <radialGradient id={id("acc-glow")}>
          <stop offset="0%" stopColor={accC} stopOpacity={0.8} />
          <stop offset="100%" stopColor={accC} stopOpacity={0} />
        </radialGradient>
        <radialGradient id={id("cheek")}>
          <stop offset="0%" stopColor={cheekC} stopOpacity={1} />
          <stop offset="50%" stopColor={cheekC} stopOpacity={0.55} />
          <stop offset="100%" stopColor={cheekC} stopOpacity={0} />
        </radialGradient>
        <filter id={id("blur")} x="-50%" y="-50%" width="200%" height="200%">
          <feGaussianBlur stdDeviation={f(R * 0.2)} />
        </filter>
        <linearGradient id={id("lens")} x1="0" y1="0" x2="0" y2="1">
          <stop offset="0%" stopColor={lensTop} />
          <stop offset="55%" stopColor={darken(accC, 0.5)} />
          <stop offset="100%" stopColor={accC} />
        </linearGradient>
      </defs>
      <g transform={head}>
        {headphones("band")}
        {top()}
        {cheeks()}
        {pill ? pillEyes() : eyeData.map(eye)}
        {capsules()}
        {brows()}
        {mouth()}
        {glasses()}
        {headphones("cups")}
      </g>
      {symbol()}
      {zees()}
      {sparkles()}
    </>
  );
}
