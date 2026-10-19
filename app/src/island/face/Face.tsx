// The synth's face on the island: SVG on a black background (the island
// draws the black; the face itself is transparent). Two levels of detail:
// compact (the closed island, about 32-37 px tall: eyes, cheeks, glasses,
// small effects) and full (the open island and the editor: brows, mouth and
// every accessory too). See motion.ts for how it moves and what it costs.

import { useEffect, useId, useMemo, useRef, useState, type CSSProperties, type ReactElement } from "react";
import { darken, drain, lighten, luminance, mix, visibleOnBlack } from "./color";
import { eyeGeom, f, heart, softStar, sparkle } from "./geometry";
import { EMOTE_LENGTH, FaceSim, LID_REST, stillFrame, type Frame, type Vec } from "./motion";
import { clampFace, type FaceConfig, type FaceState, type IdleEmote } from "./schema";

/** Below this height (px) the face draws its compact level of detail. */
export const COMPACT_BELOW = 64;

const BOX = {
  compact: { x: -68, y: -29, w: 136, h: 58 },
  full: { x: -80, y: -62, w: 160, h: 114 },
};

/** Width / height of the face box. */
export function faceAspect(compact: boolean): number {
  const b = compact ? BOX.compact : BOX.full;
  return b.w / b.h;
}

/** The width in px the face takes at a given height. */
export function faceWidth(size: number, compact = size < COMPACT_BELOW): number {
  return Math.round(size * faceAspect(compact));
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
  /** Level of detail; defaults to size < COMPACT_BELOW. */
  compact?: boolean;
  /** Draw one settled pose and never animate (thumbnails, screenshots). */
  still?: boolean;
  /** Idle with no gaze change for motion.sleepy_after_min falls asleep by itself. Default true. */
  autoSleep?: boolean;
  /** Plays an idle emote when `nonce` changes (editor preview). */
  emote?: { kind: IdleEmote; nonce: number } | null;
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

  const input = useMemo(() => ({ face, state, gaze, level, reduced, autoSleep }), [face, state, gaze, level, reduced, autoSleep]);
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

  const stillShot = useMemo(
    () =>
      still || !live
        ? stillFrame(face, state, gaze, level, still && emoteKind ? { kind: emoteKind, t: EMOTE_LENGTH[emoteKind] * 0.45, side: 1 } : null)
        : null,
    [still, live, face, state, gaze, level, emoteKind],
  );
  const frame = stillShot ?? live!;

  const box = compact ? BOX.compact : BOX.full;
  return (
    <svg
      className={props.className}
      style={{ display: "block", overflow: "visible", ...props.style }}
      width={faceWidth(size, compact)}
      height={size}
      viewBox={`${box.x} ${box.y} ${box.w} ${box.h}`}
      role="img"
      aria-label={props.label}
      aria-hidden={props.label ? undefined : true}
    >
      {drawFace(face, frame, compact, reduced, uid)}
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
const TONGUE = "#FF7D9C";
const SWEAT = "#8FD3FF";

function drawFace(face: FaceConfig, fr: Frame, compact: boolean, reduced: boolean, uid: string): ReactElement {
  const P = fr.pose;
  const dim = clamp(P.dim, 0, 1);
  const tone = (c: string) => drain(c, dim);
  const id = (name: string) => `${uid}-${name}`;
  const url = (name: string) => `url(#${id(name)})`;

  const R = 11 + face.eyes.size * 13;
  const geom = eyeGeom(face.eyes.shape, R);
  const { hx, hy } = geom;
  const d = Math.max(18 + face.eyes.spacing * 22, hx + 5);
  const flat = face.eyes.iris.size < 0.12 && face.eyes.pupil.size < 0.08;

  const sclera = tone(visibleOnBlack(face.eyes.sclera, 0.06));
  const accent = tone(visibleOnBlack(face.palette.accent, 0.14));
  const ink = tone(visibleOnBlack(face.brows.color, 0.16));
  const mouthC = tone(visibleOnBlack(face.mouth.color, 0.14));
  const accC = tone(visibleOnBlack(face.accessory_color, 0.1));
  const lidC = tone(face.eyes.lids.color);
  const irisC = tone(face.eyes.iris.color);
  const pupilC = tone(face.eyes.pupil.color);
  const cheekC = tone(face.cheeks.color);
  const glowC = flat ? sclera : accent;
  const lidVisible = luminance(lidC) > 0.025;
  const thick = compact ? 1.3 : 1;

  const glow = clamp(face.eyes.glow + P.glow, 0, 1);
  const blush = clamp(face.cheeks.blush + P.blush, 0, 1);

  // Whole-eye shift with the gaze: small for eyes with an iris, large for flat ones.
  const gx = clamp(P.gx, -1.2, 1.2);
  const gy = clamp(P.gy, -1.2, 1.2);
  const shiftX = gx * (flat ? R * 0.36 : R * 0.08);
  const shiftY = gy * (flat ? R * 0.26 : R * 0.06);

  const headK = compact ? 0.5 : 1;
  const head = `translate(${f(P.hx * headK)} ${f(P.hy * headK)}) rotate(${f(P.hr * (compact ? 0.45 : 1))} 0 ${f(R)})`;
  // Antenna and halo trail the head on a loose spring.
  const wobX = clamp((P.ax - P.hx) * 1.4 - P.hr * 0.25, -R, R);
  const wobY = clamp((P.ay - P.hy) * 1.2, -R, R);

  const eyeData = ([-1, 1] as const).map((s) => {
    const left = s < 0;
    const sc = left ? P.sL : P.sR;
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
            <circle r={f(r * 0.18)} fill={pupilC} />
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

  const shine = () => {
    const amt = face.eyes.shine;
    if (amt < 0.02) return null;
    if (flat) {
      return (
        <ellipse
          cx={f(-hx * 0.3)}
          cy={f(-hy * 0.45)}
          rx={f(hx * 0.34)}
          ry={f(hy * 0.16)}
          fill="#FFFFFF"
          opacity={amt * 0.75 * (1 - dim * 0.7)}
          transform={`rotate(-18 ${f(-hx * 0.3)} ${f(-hy * 0.45)})`}
        />
      );
    }
    const sr = irisR > 0 ? irisR : Math.max(pupilR, Math.min(hx, hy) * 0.5);
    const cx = ix - sr * 0.36 - gx * sr * 0.1;
    const cy = iy - sr * 0.4 - gy * sr * 0.08;
    const minR = compact ? 1.3 : 0.6;
    return (
      <g opacity={1 - dim * 0.7}>
        <ellipse cx={f(cx)} cy={f(cy)} rx={f(Math.max(minR, sr * 0.3))} ry={f(Math.max(minR * 0.8, sr * 0.23))} fill="#FFFFFF" opacity={amt * 0.95} transform={`rotate(-30 ${f(cx)} ${f(cy)})`} />
        {compact ? null : <circle cx={f(ix + sr * 0.32)} cy={f(iy + sr * 0.3)} r={f(sr * 0.11)} fill="#FFFFFF" opacity={amt * 0.6} />}
      </g>
    );
  };

  const lashes = (e: (typeof eyeData)[number]) => {
    if (face.eyes.lashes === "none") return null;
    const full = face.eyes.lashes === "full";
    const spec: [number, number, number][] = full
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
    const w = R * (full ? 0.085 : 0.07) * thick;
    const parts = spec.map(([fx, deg, len], i) => {
      const x = fx * hx * 0.94;
      const top = geom.top(x);
      const open = Number.isNaN(top) ? e.edgeY(x) : Math.max(e.edgeY(x), top);
      const y = lerp(open, e.lineY(Math.min(x, e.hw)), e.lineOp);
      const a = (deg * Math.PI) / 180;
      const L = R * len * (compact ? 1.15 : 1) * (1 - 0.45 * smooth((e.u - 0.25) / 0.35) * (1 - e.lineOp));
      const dx = Math.sin(a) * L;
      const dy = -Math.cos(a) * L * (1 - 2 * down);
      return <path key={i} d={`M${f(x)},${f(y)}Q${f(x + dx * 0.35)},${f(y + dy * 0.75)} ${f(x + dx)},${f(y + dy)}`} />;
    });
    let wing = null;
    if (full) {
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
    return (
      <g key={s} transform={`translate(${f(ex)} ${f(shiftY)}) scale(${f(sx)} ${f(sy)})`}>
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
        {glow > 0.01 ? <ellipse rx={f(hx * 1.9)} ry={f(hy * 1.9)} fill={url("glow")} opacity={glow * (0.4 + 0.6 * e.contentOp)} /> : null}
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
          {irisR > 0 || pupilR > 0 ? (
            <g opacity={e.irisOp} transform={`translate(${f(ix)} ${f(iy)}) scale(${f(1 - 0.16 * Math.abs(gx))} ${f(1 - 0.1 * Math.abs(gy))})`}>
              {irisR > 0 ? (
                <>
                  <circle r={f(irisR)} fill={url("iris")} />
                  {compact ? null : <circle r={f(irisR * 0.95)} fill="none" stroke={darken(irisC, 0.45)} strokeWidth={f(irisR * 0.1)} opacity={0.5} />}
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
    );
  };

  // ---------- brows, mouth, cheeks ----------

  const brows = () => {
    const style = face.brows.style;
    if (compact || style === "none") return null;
    const spec = {
      thin: { w: 0.12, arch: 0.35, angle: 0, lift: 0, half: 0.7 },
      thick: { w: 0.26, arch: 0.18, angle: 0, lift: 0, half: 0.66 },
      arched: { w: 0.15, arch: 0.75, angle: 0, lift: 0.12, half: 0.72 },
      flat: { w: 0.19, arch: 0.02, angle: 0, lift: 0, half: 0.68 },
      worried: { w: 0.15, arch: 0.22, angle: 13, lift: 0.05, half: 0.68 },
    }[style];
    return (
      <g fill="none" stroke={ink} strokeLinecap="round" strokeWidth={f(R * spec.w)}>
        {eyeData.map((e) => {
          const s = e.s;
          const left = s < 0;
          const lift = (left ? P.bLl : P.bRl) + spec.lift;
          const angle = (left ? P.bLa : P.bRa) + spec.angle - face.eyes.tilt * 8;
          const lidDown = (left ? P.uL : P.uR) - face.eyes.lids.rest * LID_REST;
          const cy = shiftY * 0.4 - hy * e.sc - R * 0.42 - 3 - lift * R * 0.42 + Math.max(0, lidDown) * R * 0.32 + e.blink * R * 0.05;
          const cx = s * (d + R * 0.06) + shiftX * 0.4;
          const half = R * spec.half;
          const dy = Math.tan((clamp(angle, -35, 35) * Math.PI) / 180) * half;
          const innerX = cx - s * half;
          const outerX = cx + s * half;
          const innerY = cy - dy;
          const outerY = cy + dy * 0.5;
          const midY = (innerY + outerY) / 2 - spec.arch * R * 0.3;
          const ctrlY = 2 * midY - (innerY + outerY) / 2;
          return <path key={s} d={`M${f(innerX)},${f(innerY)}Q${f(cx)},${f(ctrlY)} ${f(outerX)},${f(outerY)}`} />;
        })}
      </g>
    );
  };

  const mouthY = Math.max(hy, R * 0.9) + R * 0.52 + 2;

  const mouth = () => {
    const style = face.mouth.style;
    if (compact || style === "none") return null;
    const sm = clamp(P.smile, -1, 1);
    const op = clamp(P.open, 0, 1.2);
    const x0 = P.mouthX * R * 0.3;
    const y0 = mouthY;
    const lw = Math.max(1.5, R * 0.12);
    const inside = darken(mouthC, 0.72);
    const tongueC = tone(TONGUE);
    const tongue =
      P.tongue > 0.02 ? (
        <ellipse cx={f(x0 + R * 0.24)} cy={f(y0 + R * 0.1 + R * 0.1 * P.tongue)} rx={f(R * 0.13)} ry={f(R * 0.17 * P.tongue)} fill={tongueC} />
      ) : null;

    if (style === "dot") {
      const r = R * 0.11 * (1 + op * 1.7);
      if (op < 0.1) return <circle cx={f(x0)} cy={f(y0)} r={f(r)} fill={mouthC} />;
      return <ellipse cx={f(x0)} cy={f(y0 + r * 0.2)} rx={f(r * 0.85)} ry={f(r * 1.1)} fill={inside} stroke={mouthC} strokeWidth={f(lw * 0.9)} />;
    }

    if (style === "cat") {
      const w = R * 0.42;
      const depth = R * (0.17 + Math.max(0, sm) * 0.08);
      const y = y0 - R * 0.06;
      const lip = `M${f(x0 - w)},${f(y - depth * 0.35)}Q${f(x0 - w / 2)},${f(y + depth * 1.2)} ${f(x0)},${f(y)}Q${f(x0 + w / 2)},${f(y + depth * 1.2)} ${f(x0 + w)},${f(y - depth * 0.35)}`;
      return (
        <g>
          {op > 0.05 ? (
            <ellipse cx={f(x0)} cy={f(y + depth * 0.5 + op * R * 0.22)} rx={f(w * 0.42)} ry={f(op * R * 0.34 + R * 0.04)} fill={inside} stroke={mouthC} strokeWidth={f(lw * 0.8)} />
          ) : null}
          {tongue}
          <path d={lip} fill="none" stroke={mouthC} strokeWidth={f(lw)} strokeLinecap="round" strokeLinejoin="round" />
        </g>
      );
    }

    const w = R * (style === "line" ? 0.4 : 0.5);
    const yc = y0 - sm * R * 0.15;
    const mid = y0 + sm * R * 0.15;
    const tiltY = P.mouthX * R * 0.12;
    const lx = x0 - w;
    const rx = x0 + w;
    const ly = yc + tiltY;
    const ry = yc - tiltY;
    const cU = 2 * mid - (ly + ry) / 2;
    if (op < 0.04) {
      return (
        <g>
          {tongue}
          <path d={`M${f(lx)},${f(ly)}Q${f(x0)},${f(cU)} ${f(rx)},${f(ry)}`} fill="none" stroke={mouthC} strokeWidth={f(lw)} strokeLinecap="round" />
        </g>
      );
    }
    const lowMid = mid + op * R * 0.8;
    const cL = 2 * lowMid - (ly + ry) / 2;
    const shape = `M${f(lx)},${f(ly)}Q${f(x0)},${f(cU)} ${f(rx)},${f(ry)}Q${f(x0)},${f(cL)} ${f(lx)},${f(ly)}Z`;
    return (
      <g>
        <clipPath id={id("m")}>
          <path d={shape} />
        </clipPath>
        <path d={shape} fill={inside} />
        <ellipse cx={f(x0)} cy={f(lowMid)} rx={f(w * 0.62)} ry={f(op * R * 0.36)} fill={tongueC} clipPath={url("m")} />
        <path d={shape} fill="none" stroke={mouthC} strokeWidth={f(lw * 0.9)} strokeLinejoin="round" />
      </g>
    );
  };

  const cheeks = () => {
    if (blush < 0.01) return null;
    const cy = hy * 0.72 + R * (compact ? 0.16 : 0.3);
    return (
      <g opacity={Math.min(1, blush * 1.25)}>
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

  // Glasses ride the nose: pushed up, slid down or askew by the pose.
  const glassesAt = (H: number) => `translate(${f(shiftX * 0.22)} ${f(shiftY * 0.22 - P.gl * H)}) rotate(${f(P.glr)} 0 ${f(-H * 0.3)})`;

  const glasses = (): ReactElement | null => {
    if (acc === "glasses" || acc === "round_glasses") {
      const round = acc === "round_glasses";
      const fw = hx + R * 0.34;
      const fh = hy + R * 0.3;
      const rr = Math.max(hx, hy) + R * 0.32;
      const inner = d - (round ? rr : fw);
      return (
        <g fill="none" stroke={accC} strokeWidth={f(accLw)} strokeLinecap="round" transform={glassesAt((hy + R * 0.3) * 0.6)}>
          {[-1, 1].map((s) =>
            round ? (
              <circle key={s} cx={f(s * d)} cy={0} r={f(rr)} fill="#FFFFFF" fillOpacity={0.05} />
            ) : (
              <rect key={s} x={f(s * d - fw)} y={f(-fh)} width={f(2 * fw)} height={f(2 * fh)} rx={f(R * 0.5)} fill="#FFFFFF" fillOpacity={0.05} />
            ),
          )}
          <path d={`M${f(-inner)},${f(-hy * 0.25)}Q0,${f(-hy * 0.5)} ${f(inner)},${f(-hy * 0.25)}`} strokeWidth={f(accLw * 0.85)} />
          {[-1, 1].map((s) => {
            const gx0 = s * d - (round ? rr : fw) * 0.45;
            return <path key={`g${s}`} d={`M${f(gx0)},${f(-hy * 0.15)}L${f(gx0 + R * 0.32)},${f(-hy * 0.62)}`} stroke="#FFFFFF" strokeOpacity={0.35} strokeWidth={f(R * 0.08)} />;
          })}
          {compact
            ? null
            : [-1, 1].map((s) => {
                const x = s * (d + (round ? rr : fw));
                return <path key={`t${s}`} d={`M${f(x)},${f(-hy * 0.35)}L${f(x + s * R * 0.55)},${f(-hy * 0.55)}`} />;
              })}
        </g>
      );
    }
    if (acc === "sunglasses") {
      const W = hx + R * 0.38;
      const H = hy + R * 0.28;
      const lens = `M${f(-0.95 * W)},${f(-0.72 * H)}C${f(-0.95 * W)},${f(-1.02 * H)} ${f(1.05 * W)},${f(-1.08 * H)} ${f(1.06 * W)},${f(-0.62 * H)}C${f(1.08 * W)},${f(0.45 * H)} ${f(0.6 * W)},${f(1.0 * H)} 0,${f(1.0 * H)}C${f(-0.62 * W)},${f(1.0 * H)} ${f(-1.0 * W)},${f(0.4 * H)} ${f(-0.95 * W)},${f(-0.72 * H)}Z`;
      const rim = lighten(accC, 0.3);
      const inner = d - 0.95 * W;
      return (
        <g transform={glassesAt(H * 1.6)}>
          {[-1, 1].map((s) => (
            <g key={s} transform={`translate(${f(s * d)} 0) scale(${s} 1)`}>
              <path d={lens} fill={url("lens")} opacity={0.82} />
              <path d={lens} fill="none" stroke={rim} strokeWidth={f(R * 0.08 * thick)} strokeOpacity={0.9} />
              <path
                d={`M${f(-0.62 * W)},${f(-0.35 * H)}L${f(-0.28 * W)},${f(-0.86 * H)}L${f(-0.08 * W)},${f(-0.86 * H)}L${f(-0.44 * W)},${f(-0.3 * H)}Z`}
                fill="#FFFFFF"
                opacity={0.32}
              />
            </g>
          ))}
          <path d={`M${f(-inner)},${f(-0.72 * H)}Q0,${f(-0.98 * H)} ${f(inner)},${f(-0.72 * H)}`} fill="none" stroke={rim} strokeWidth={f(R * 0.12 * thick)} strokeLinecap="round" />
          {compact
            ? null
            : [-1, 1].map((s) => (
                <path key={`t${s}`} d={`M${f(s * (d + 1.05 * W))},${f(-0.6 * H)}L${f(s * (d + 1.05 * W + R * 0.5))},${f(-0.72 * H)}`} stroke={rim} strokeWidth={f(R * 0.1)} strokeLinecap="round" />
              ))}
        </g>
      );
    }
    return null;
  };

  const top = (): ReactElement | null => {
    if (compact) return null;
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

  const headphones = (part: "band" | "cups"): ReactElement | null => {
    if (compact || acc !== "headphones") return null;
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
        />
      );
    }
    const lvl = fr.state === "listening" ? fr.level : 0;
    return (
      <g>
        {[-1, 1].map((s) => (
          <g key={s} transform={`translate(${f(s * cx)} ${f(R * 0.12)}) scale(${f(1 + lvl * 0.06)})`}>
            {lvl > 0.02 ? <rect x={f(-R * 0.6)} y={f(-R * 1.0)} width={f(R * 1.2)} height={f(R * 2.0)} rx={f(R * 0.55)} fill={url("acc-glow")} opacity={lvl} /> : null}
            <rect x={f(-R * 0.36)} y={f(-R * 0.76)} width={f(R * 0.72)} height={f(R * 1.52)} rx={f(R * 0.34)} fill={accC} />
            <rect x={f(s > 0 ? -R * 0.36 : R * 0.12)} y={f(-R * 0.6)} width={f(R * 0.24)} height={f(R * 1.2)} rx={f(R * 0.12)} fill={darken(accC, 0.45)} />
          </g>
        ))}
      </g>
    );
  };

  // ---------- effects ----------

  const listeningArcs = () => {
    if (fr.state !== "listening" || (acc === "headphones" && !compact)) return null;
    return (
      <g fill="none" stroke={accent} strokeLinecap="round" strokeWidth={f(R * 0.11 * thick)}>
        {[-1, 1].flatMap((s) =>
          [0, 1].map((k) => {
            const r = hx + R * (0.5 + 0.42 * k);
            const a = (38 * Math.PI) / 180;
            const lvl = k === 0 ? fr.level : fr.levelSlow;
            const op = 0.16 + 0.84 * clamp(lvl * 1.3, 0, 1);
            const cx = s * d + shiftX;
            const x = cx + s * r * Math.cos(a);
            const y1 = -r * Math.sin(a);
            const y2 = r * Math.sin(a);
            return <path key={`${s}${k}`} d={`M${f(x)},${f(y1)}A${f(r)},${f(r)} 0 0 ${s > 0 ? 1 : 0} ${f(x)},${f(y2)}`} opacity={op} />;
          }),
        )}
      </g>
    );
  };

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
      const y = -hy * 0.3 - u * R * (compact ? 0.9 : 2.0);
      const s = R * (0.16 + u * 0.18) * (compact ? 1.2 : 1);
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

  const sparkles = () => {
    if (fr.state !== "done" || fr.st > 1.5) return null;
    const spots: [number, number, number][] = [
      [-d - hx * 0.85, -hy * 0.75, 0],
      [d + hx * 0.95, -hy * 0.3, 0.12],
      [compact ? 0 : d * 0.15, compact ? -hy * 0.95 : eyeTop - R * 0.75, 0.24],
    ];
    const c = lighten(accent, 0.25);
    return (
      <g fill={c}>
        {spots.map(([x, y, delay], i) => {
          const age = fr.st - delay;
          const life = 0.8;
          if (age <= 0 || age >= life) return null;
          const k = Math.sin((Math.PI * age) / life);
          const r = R * 0.34 * k * (compact ? 1.1 : 1);
          return <path key={i} d={sparkle(x, y, r)} transform={`rotate(${f(age * 80)} ${f(x)} ${f(y)})`} />;
        })}
      </g>
    );
  };

  const sweat = () => {
    if (compact || fr.state !== "error") return null;
    const st = fr.st;
    const op = smooth(st / 0.25) * (1 - smooth((st - 2.4) / 0.5));
    if (op < 0.01) return null;
    const r = R * 0.17;
    const x = d + hx * 0.78;
    const y = eyeTop - R * 0.05 + smooth(st / 1.4) * R * 0.35;
    const c = tone(SWEAT);
    return (
      <g transform={`translate(${f(x)} ${f(y)})`} opacity={op}>
        <path d={`M0,${f(-r * 1.7)}C${f(r * 0.35)},${f(-r * 0.8)} ${f(r)},${f(-r * 0.25)} ${f(r)},${f(r * 0.35)}A${f(r)},${f(r)} 0 0 1 ${f(-r)},${f(r * 0.35)}C${f(-r)},${f(-r * 0.25)} ${f(-r * 0.35)},${f(-r * 0.8)} 0,${f(-r * 1.7)}Z`} fill={c} />
        <ellipse cx={f(-r * 0.35)} cy={f(r * 0.2)} rx={f(r * 0.2)} ry={f(r * 0.3)} fill="#FFFFFF" opacity={0.6} />
      </g>
    );
  };

  const lensTop = darken(accC, 0.85);
  return (
    <>
      <defs>
        <radialGradient id={id("iris")} cx="50%" cy="62%" r="60%">
          <stop offset="0%" stopColor={lighten(irisC, 0.38)} />
          <stop offset="58%" stopColor={irisC} />
          <stop offset="100%" stopColor={darken(irisC, 0.38)} />
        </radialGradient>
        <radialGradient id={id("flat")} cx="45%" cy="40%" r="65%">
          <stop offset="0%" stopColor={lighten(sclera, 0.6)} />
          <stop offset="55%" stopColor={sclera} />
          <stop offset="100%" stopColor={darken(sclera, 0.22)} />
        </radialGradient>
        <radialGradient id={id("glow")}>
          <stop offset="0%" stopColor={glowC} stopOpacity={0.6} />
          <stop offset="45%" stopColor={glowC} stopOpacity={0.22} />
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
        {eyeData.map(eye)}
        {brows()}
        {mouth()}
        {glasses()}
        {headphones("cups")}
        {sweat()}
      </g>
      {listeningArcs()}
      {zees()}
      {sparkles()}
    </>
  );
}
