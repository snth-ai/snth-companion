// What a pill face wears: brows, cheeks and every accessory, fitted to the
// tall beans and riding with the turned face.
//
// The beans are not slid around: the rig places each one on a turned head
// (rig.ts placePill). Everything drawn here goes through the same mapping
// (rig.ts pillWarp), in the same frame, so it turns, shifts and foreshortens
// with the beans instead of floating in place. Two kinds of anchoring:
//
//   per eye    Glasses lenses, sunglasses and brows sit on one bean: they get
//              that bean's position and size (the near eye's lens grows with
//              it, the far one narrows). They do not follow a bean's own
//              squash (blink, squint, startle), as a real frame would not.
//   on the face Cheeks, the top accessories (ears, horns, bow, halo, crown,
//              antenna) and headphones are placed at their point of the face
//              plane: mapped there with the local scale, so the whole face
//              turns as one surface.
//
// Breathing, the state's head offset, tilt and the nod act on the group all
// of this is drawn in (Face.tsx), so they need nothing here.
//
// The closed island (34 px, det 0) has about 7 px between the top of the
// beans and the top of the screen. There: glasses and round glasses show
// with thin frames and no arms; sunglasses show with lighter lenses so the
// beans read through; headphones show their cups only; brows show only in
// the states they help tell apart (as on every face); cat ears, horns, bow,
// halo, crown and antenna are hidden (they fade in as the island opens).

import type { ReactElement } from "react";
import { darken, lighten, mix } from "./color";
import { f } from "./geometry";
import { LID_REST, type Frame } from "./motion";
import type { FaceWarp, Vec } from "./rig";
import type { FaceConfig, FaceState } from "./schema";

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));
const lerp = (a: number, b: number, t: number) => a + (b - a) * t;
/** Scales need finer steps than f's 0.01 (see Face.tsx). */
const fs = (n: number) => (Math.round(n * 10000) / 10000).toString();

/** Compact brows appear only where they carry the state (as in Face.tsx). */
const BROW_STATES: ReadonlySet<FaceState> = new Set(["thinking", "error", "approval"]);

export interface WearEye {
  s: -1 | 1;
  /** The eye's scale pop (sL / sR). */
  sc: number;
  /** Lifted (-) or lowered (+), face units. */
  lift: number;
}

export interface WearInput {
  face: FaceConfig;
  fr: Frame;
  det: number;
  tiny: boolean;
  /** Face units per screen pixel. */
  px: number;
  reduced: boolean;
  R: number;
  hx: number;
  hy: number;
  d: number;
  thick: number;
  warp: FaceWarp;
  /** The visible part of the face, for the glasses' room to move. */
  viewTop: number;
  viewBottom: number;
  headK: number;
  eyes: [WearEye, WearEye];
  ink: string;
  accC: string;
  cheekC: string;
  blush: number;
  off: number;
  /** The gloss highlight's slide (rig.ts PillPlacement.hl). */
  hl: Vec;
  /** Antenna and halo wobble (Face.tsx wobX / wobY). */
  wob: Vec;
  url: (name: string) => string;
  id: (name: string) => string;
}

export interface Wear {
  /** Behind the eyes: headphone band, top accessories, cheeks. */
  back: ReactElement | null;
  brows: ReactElement | null;
  /** In front of everything: glasses, sunglasses, headphone cups. */
  front: ReactElement | null;
}

/** A frame for one lens: a centre at rest (cx, cy) and where the turned
 * face puts it (X, Y), with the local scale and a tilt in degrees. */
interface LensFrame {
  cx: number;
  cy: number;
  X: number;
  Y: number;
  sx: number;
  sy: number;
  rot: number;
}

const frameT = (F: LensFrame) =>
  `translate(${f(F.X)} ${f(F.Y)})${F.rot ? ` rotate(${f(F.rot)})` : ""} scale(${fs(F.sx)} ${fs(F.sy)}) translate(${f(-F.cx)} ${f(-F.cy)})`;

/** A rest-space point through a lens frame (the same as frameT). */
function framePt(F: LensFrame, x: number, y: number): [number, number] {
  const dx = (x - F.cx) * F.sx;
  const dy = (y - F.cy) * F.sy;
  const a = (F.rot * Math.PI) / 180;
  const c = Math.cos(a);
  const s = Math.sin(a);
  return [F.X + dx * c - dy * s, F.Y + dx * s + dy * c];
}

const pt = (p: [number, number]) => `${f(p[0])},${f(p[1])}`;

export function pillWear(i: WearInput): Wear {
  const { face, fr, det, tiny, px, R, hx, hy, d, warp, accC } = i;
  const P = fr.pose;
  const acc = face.accessory;
  const framed = acc === "glasses" || acc === "round_glasses" || acc === "sunglasses";

  /** Draws `inner` (rest coordinates) with the point (x, y) carried to where
   * the turned face puts it, at the local scale there. */
  const at = (x: number, y: number) => {
    const w = warp(x, y);
    return `translate(${f(w.x)} ${f(w.y)}) scale(${fs(w.sx)} ${fs(w.sy)}) translate(${f(-x)} ${f(-y)})`;
  };
  const mp = (x: number, y: number) => {
    const w = warp(x, y);
    return `${f(w.x)},${f(w.y)}`;
  };

  // ---------- brows ----------

  const brows = (): ReactElement | null => {
    const style = face.brows.style;
    if (style === "none") return null;
    const stateBrow = BROW_STATES.has(fr.state) ? 1 : 0;
    const op = Math.max(det, stateBrow * (1 - i.off));
    if (op < 0.01) return null;
    const spec = {
      thin: { w: 0.12, arch: 0.35, angle: 0, lift: 0, half: 0.7 },
      thick: { w: 0.26, arch: 0.18, angle: 0, lift: 0, half: 0.66 },
      arched: { w: 0.15, arch: 0.75, angle: 0, lift: 0.12, half: 0.72 },
      flat: { w: 0.19, arch: 0.02, angle: 0, lift: 0, half: 0.68 },
      worried: { w: 0.15, arch: 0.22, angle: 13, lift: 0.05, half: 0.68 },
    }[style];
    const width = lerp(1.4 * px, R * spec.w, det);
    // A bean is narrower than a round eye: the brow spans it and a little more.
    const half = lerp(3.2 * px, hx * spec.half * 1.55, det);
    // Under the brows, the headphone band or the base of ears, horns, the
    // crown and the antenna (there is no room to lift them clear): a black
    // edge keeps the two apart, the brows in front.
    const edge = det > 0.01 && (acc === "headphones" || acc === "cat_ears" || acc === "horns" || acc === "crown" || acc === "antenna" || acc === "bow");
    return (
      <g fill="none" stroke={i.ink} strokeLinecap="round" strokeWidth={f(width)} opacity={op}>
        {i.eyes.map((e) => {
          const s = e.s;
          const left = s < 0;
          const lift = (left ? P.bLl : P.bRl) + spec.lift;
          const angle = (left ? P.bLa : P.bRa) + spec.angle - face.eyes.tilt * 8;
          // The lids squash a bean from the top: the brow follows part of it.
          const lidDown = (left ? P.uL : P.uR) - face.eyes.lids.rest * LID_REST;
          // Over glasses the brow clears the frame's top rim.
          const gap = lerp(2.2 * px, R * 0.3 + 1.5, det) + (framed ? lerp(2.4 * px, R * 0.3, det) : 0);
          const liftK = lerp(1.5 * px, R * 0.36, det);
          const top = -hy * e.sc * Math.max(0.3, 1 + P.stretch);
          const cy = e.lift + top - gap - lift * liftK + Math.max(0, lidDown) * hy * 1.1 * det;
          const cx = s * (d + hx * 0.06);
          const dy = Math.tan((clamp(angle, -35, 35) * Math.PI) / 180) * half;
          const innerX = cx - s * half;
          const outerX = cx + s * half;
          const innerY = cy - dy;
          const outerY = cy + dy * 0.5;
          const midY = (innerY + outerY) / 2 - spec.arch * R * 0.3 * (half / (R * spec.half));
          const ctrlY = 2 * midY - (innerY + outerY) / 2;
          return (
            <g key={s} transform={at(s * d, 0)}>
              {edge ? <path d={`M${f(innerX)},${f(innerY)}Q${f(cx)},${f(ctrlY)} ${f(outerX)},${f(outerY)}`} stroke="#000000" strokeWidth={f(width + R * 0.22)} /> : null}
              <path d={`M${f(innerX)},${f(innerY)}Q${f(cx)},${f(ctrlY)} ${f(outerX)},${f(outerY)}`} />
            </g>
          );
        })}
      </g>
    );
  };

  // ---------- cheeks ----------

  /** Below and outside each bean, hugging its lower outer corner (drawn
   * behind the beans, so the corner covers the inner part). */
  const cheeks = (): ReactElement | null => {
    if (i.blush < 0.01) return null;
    const cy = hy * 0.86;
    return (
      <g opacity={Math.min(1, i.blush * 1.25) * (1 - i.off)}>
        {[-1, 1].map((s) => {
          const cx = s * (d + hx * 0.8);
          return <ellipse key={s} transform={at(cx, cy)} cx={f(cx)} cy={f(cy)} rx={f(R * 0.48)} ry={f(R * 0.28)} fill={i.url("cheek")} />;
        })}
      </g>
    );
  };

  // ---------- glasses ----------

  /** The lenses' frames. Glasses ride the nose: pushed up or slid down by
   * the pose (P.gl, in lens heights `H`), never more than 3 px on a closed
   * island nor closer than 2 px to the edge, and tilted as a pair (P.glr)
   * around a point above the nose. */
  const lensFrames = (H: number, half: number): [LensFrame, LensFrame] => {
    const want = P.gl * H;
    const maxUp = det < 0.5 ? 3 * px : H * 0.9;
    const roomUp = -half - (i.viewTop + 2 * px) + P.hy * i.headK;
    const roomDown = i.viewBottom - 2 * px - half - P.hy * i.headK;
    const lift = clamp(want, -Math.max(0, roomDown), Math.max(0, Math.min(maxUp, roomUp)));
    const rot = P.glr;
    const a = (rot * Math.PI) / 180;
    const pivotY = -H * 0.3;
    const make = (s: -1 | 1): LensFrame => {
      const x0 = s * d;
      const y0 = -lift - pivotY;
      const cx = x0 * Math.cos(a) - y0 * Math.sin(a);
      const cy = pivotY + x0 * Math.sin(a) + y0 * Math.cos(a);
      const w = warp(cx, cy);
      return { cx, cy, X: w.x, Y: w.y, sx: w.sx, sy: w.sy, rot };
    };
    return [make(-1), make(1)];
  };

  const accLw = Math.max(1.4, R * 0.13) * i.thick;
  /** A short glint on clear lenses, lit from the top left like the beans'
   * gloss, sliding against the turn: it says which way the head points. */
  const glint = (F: LensFrame, a: number, b: number) => {
    if (det < 0.5) return null;
    const gx = F.cx + (-0.42 + i.hl.x * 0.9) * a;
    const gy = F.cy + (-0.5 + i.hl.y * 0.8) * b;
    return (
      <path
        d={`M${f(gx - a * 0.1)},${f(gy + b * 0.22)}L${f(gx + a * 0.12)},${f(gy - b * 0.12)}`}
        stroke="#FFFFFF"
        strokeWidth={f(accLw * 0.8)}
        strokeLinecap="round"
        opacity={0.32 * det}
      />
    );
  };

  const glasses = (): ReactElement | null => {
    const round = acc === "round_glasses";
    // Rectangular: a tall soft lens around the bean, narrow enough that the
    // smile between the beans stays clear of the inner rims (a wider lens
    // closes the mouth and the bridge into a ring). Round: a circle just
    // clear of the bean's corners, which curves away from the smile.
    const a = round ? hy + R * 0.2 : hx + R * 0.28;
    const b = round ? a : hy + R * 0.26;
    const lw = tiny ? Math.max(1.1 * px, accLw * 0.8) : accLw;
    const F = lensFrames(b * 0.6, b);
    // Bridge: from each lens's inner edge, a little above the middle,
    // arching over the nose.
    const by = round ? -Math.sin(0.62) * a : -b * 0.5;
    const bx = round ? Math.cos(0.62) * a : a;
    const L = framePt(F[0], F[0].cx + bx, F[0].cy + by);
    const Rr = framePt(F[1], F[1].cx - bx, F[1].cy + by);
    const mid = warp(0, (F[0].cy + F[1].cy) / 2 + by - b * 0.16);
    const ctrl: [number, number] = [2 * mid.x - (L[0] + Rr[0]) / 2, 2 * mid.y - (L[1] + Rr[1]) / 2];
    return (
      <g fill="none" stroke={accC} strokeWidth={f(lw)} strokeLinecap="round">
        {F.map((Fr, k) => {
          const s = k === 0 ? -1 : 1;
          return (
            <g key={s} transform={frameT(Fr)}>
              {round ? (
                <circle cx={f(Fr.cx)} cy={f(Fr.cy)} r={f(a)} fill="#FFFFFF" fillOpacity={0.04} />
              ) : (
                <rect x={f(Fr.cx - a)} y={f(Fr.cy - b)} width={f(2 * a)} height={f(2 * b)} rx={f(R * 0.55)} fill="#FFFFFF" fillOpacity={0.04} />
              )}
              {glint(Fr, a, b)}
              {det > 0.01 ? (
                <path
                  d={`M${f(Fr.cx + s * a * (round ? 0.97 : 1))},${f(Fr.cy - b * 0.32)}L${f(Fr.cx + s * (a + R * 0.5))},${f(Fr.cy - b * 0.5)}`}
                  opacity={det}
                />
              ) : null}
            </g>
          );
        })}
        <path d={`M${pt(L)}Q${pt(ctrl)} ${pt(Rr)}`} strokeWidth={f(lw * 0.85)} />
      </g>
    );
  };

  const sunglasses = (): ReactElement | null => {
    const W = hx + R * 0.45;
    const H = hy + R * 0.3;
    const lens = `M${f(-0.95 * W)},${f(-0.72 * H)}C${f(-0.95 * W)},${f(-1.02 * H)} ${f(1.05 * W)},${f(-1.08 * H)} ${f(1.06 * W)},${f(-0.62 * H)}C${f(1.08 * W)},${f(0.45 * H)} ${f(0.6 * W)},${f(1.0 * H)} 0,${f(1.0 * H)}C${f(-0.62 * W)},${f(1.0 * H)} ${f(-1.0 * W)},${f(0.4 * H)} ${f(-0.95 * W)},${f(-0.72 * H)}Z`;
    const rim = lighten(accC, 0.3);
    // Pushed up only a little: dark glass has to keep covering the beans.
    const F = lensFrames(H * 0.45, H);
    // The beans glow through the dark glass: their place and size behind it
    // still say where the eyes look. Lighter on the closed island.
    const lensOp = lerp(0.6, 0.84, det);
    const rimW = Math.max(tiny ? 1.1 * px : 0, R * 0.08 * i.thick);
    const L = framePt(F[0], F[0].cx + 0.95 * W, F[0].cy - 0.72 * H);
    const Rr = framePt(F[1], F[1].cx - 0.95 * W, F[1].cy - 0.72 * H);
    const mid = warp(0, (F[0].cy + F[1].cy) / 2 - 0.98 * H);
    const ctrl: [number, number] = [2 * mid.x - (L[0] + Rr[0]) / 2, 2 * mid.y - (L[1] + Rr[1]) / 2];
    return (
      <g>
        {F.map((Fr, k) => {
          const s = k === 0 ? -1 : 1;
          const clip = i.id(k === 0 ? "slc-l" : "slc-r");
          // The reflection: a bright slant at the top left of each lens that
          // slides against the turn, in screen terms (not mirrored).
          const rx = (-0.5 + i.hl.x * 1.1) * W;
          const ry = i.hl.y * 0.8 * H;
          return (
            <g key={s} transform={frameT(Fr)}>
              <g transform={`translate(${f(Fr.cx)} ${f(Fr.cy)})`}>
                <clipPath id={clip}>
                  <path d={lens} transform={`scale(${s} 1)`} />
                </clipPath>
                <path d={lens} transform={`scale(${s} 1)`} fill={i.url("plens")} opacity={lensOp} />
                {det > 0.01 ? (
                  <path
                    clipPath={`url(#${clip})`}
                    d={`M${f(rx - 0.12 * W)},${f(ry + 0.05 * H)}L${f(rx + 0.22 * W)},${f(ry - 0.95 * H)}L${f(rx + 0.44 * W)},${f(ry - 0.95 * H)}L${f(rx + 0.1 * W)},${f(ry + 0.1 * H)}Z`}
                    fill="#FFFFFF"
                    opacity={0.22 * det}
                  />
                ) : null}
                <path d={lens} transform={`scale(${s} 1)`} fill="none" stroke={rim} strokeWidth={f(rimW)} strokeOpacity={0.9} />
                {det > 0.01 ? (
                  <path
                    d={`M${f(s * 1.05 * W)},${f(-0.6 * H)}L${f(s * (1.05 * W + R * 0.5))},${f(-0.72 * H)}`}
                    stroke={rim}
                    strokeWidth={f(R * 0.1)}
                    strokeLinecap="round"
                    opacity={det}
                  />
                ) : null}
              </g>
            </g>
          );
        })}
        <path d={`M${pt(L)}Q${pt(ctrl)} ${pt(Rr)}`} fill="none" stroke={rim} strokeWidth={f(Math.max(rimW, R * 0.12 * i.thick))} strokeLinecap="round" />
      </g>
    );
  };

  // ---------- on top of the head ----------

  /** Top accessories sit in the band between the tops of the beans and the
   * top of the open island, low enough that a look up, a hop and a breath
   * do not push them out of it. */
  const topY = -hy;

  /** `inner` drawn around the rest point (x, y), carried by the face. */
  const on = (x: number, y: number, inner: ReactElement, key?: number) => (
    <g key={key} transform={at(x, y)}>
      {inner}
    </g>
  );

  const top = (): ReactElement | null => {
    switch (acc) {
      case "cat_ears": {
        const inner = mix(i.cheekC, "#FFFFFF", 0.25);
        const k = 0.74;
        return (
          <g>
            {([-1, 1] as const).map((s) => {
              const x = s * (d + hx * 0.4);
              const y = topY - R * 0.52;
              return on(
                x,
                y,
                <g transform={`translate(${f(x)} ${f(y)}) rotate(${f(s * (20 + P.ear * 0.6))}) scale(${s * k} ${k})`}>
                  <path
                    d={`M${f(-R * 0.78)},${f(R * 0.05)}Q${f(-R * 0.55)},${f(-R * 0.7)} ${f(-R * 0.05)},${f(-R * 1.22)}Q${f(R * 0.12)},${f(-R * 1.36)} ${f(R * 0.2)},${f(-R * 1.15)}Q${f(R * 0.55)},${f(-R * 0.55)} ${f(R * 0.72)},${f(R * 0.05)}Z`}
                    fill={accC}
                  />
                  <path d={`M${f(-R * 0.42)},${f(-R * 0.12)}Q${f(-R * 0.28)},${f(-R * 0.6)} ${f(0)},${f(-R * 0.92)}Q${f(R * 0.25)},${f(-R * 0.5)} ${f(R * 0.38)},${f(-R * 0.12)}Z`} fill={inner} />
                </g>,
                s,
              );
            })}
          </g>
        );
      }
      case "bow": {
        // A clip on the top outer corner of the right bean.
        const fold = darken(accC, 0.28);
        const wing = `M0,0C${f(-R * 0.3)},${f(-R * 0.6)} ${f(-R * 1.05)},${f(-R * 0.72)} ${f(-R * 0.98)},${f(-R * 0.02)}C${f(-R * 0.95)},${f(R * 0.6)} ${f(-R * 0.32)},${f(R * 0.42)} 0,0Z`;
        const x = d + hx + R * 0.12;
        const y = topY - R * 0.3;
        return on(
          x,
          y,
          <g transform={`translate(${f(x)} ${f(y)}) rotate(${f(24 + P.ear * 0.5)}) scale(0.64)`}>
            <path d={`M${f(-R * 0.08)},${f(R * 0.1)}L${f(-R * 0.42)},${f(R * 0.82)}L${f(-R * 0.14)},${f(R * 0.72)}Z`} fill={fold} />
            <path d={`M${f(R * 0.08)},${f(R * 0.1)}L${f(R * 0.4)},${f(R * 0.8)}L${f(R * 0.14)},${f(R * 0.7)}Z`} fill={fold} />
            <path d={wing} fill={accC} />
            <path d={wing} fill={accC} transform="scale(-1 1)" />
            <path d={`M${f(-R * 0.25)},${f(-R * 0.12)}Q${f(-R * 0.6)},${f(-R * 0.1)} ${f(-R * 0.75)},${f(R * 0.05)}`} fill="none" stroke={fold} strokeWidth={f(R * 0.06)} strokeLinecap="round" />
            <path d={`M${f(R * 0.25)},${f(-R * 0.12)}Q${f(R * 0.6)},${f(-R * 0.1)} ${f(R * 0.75)},${f(R * 0.05)}`} fill="none" stroke={fold} strokeWidth={f(R * 0.06)} strokeLinecap="round" />
            <ellipse rx={f(R * 0.24)} ry={f(R * 0.28)} fill={lighten(accC, 0.12)} />
          </g>,
        );
      }
      case "halo": {
        const bob = i.reduced ? 0 : Math.sin(fr.t * 2.1) * 0.7;
        const cy = topY - R * 1.0 + i.wob.y * 0.6 + bob;
        const cx = i.wob.x * 0.5;
        const rx = d * 0.8;
        return on(
          cx,
          cy,
          <g fill="none" stroke={accC}>
            <ellipse cx={f(cx)} cy={f(cy)} rx={f(rx)} ry={f(R * 0.24)} strokeWidth={f(R * 0.3)} opacity={0.5} filter={i.url("blur")} />
            <ellipse cx={f(cx)} cy={f(cy)} rx={f(rx)} ry={f(R * 0.24)} strokeWidth={f(R * 0.15)} />
            <path d={`M${f(cx - rx * 0.7)},${f(cy + R * 0.16)}Q${f(cx)},${f(cy + R * 0.38)} ${f(cx + rx * 0.7)},${f(cy + R * 0.16)}`} stroke={lighten(accC, 0.6)} strokeWidth={f(R * 0.05)} strokeLinecap="round" />
          </g>,
        );
      }
      case "horns": {
        const hi = lighten(accC, 0.35);
        const k = 0.78;
        return (
          <g>
            {([-1, 1] as const).map((s) => {
              const x = s * (d + hx * 0.3);
              const y = topY - R * 0.5;
              return on(
                x,
                y,
                <g transform={`translate(${f(x)} ${f(y)}) rotate(${f(s * 16)}) scale(${s * k} ${k})`}>
                  <path
                    d={`M${f(-R * 0.3)},${f(R * 0.08)}C${f(-R * 0.26)},${f(-R * 0.55)} ${f(R * 0.02)},${f(-R * 1.0)} ${f(R * 0.52)},${f(-R * 1.22)}C${f(R * 0.3)},${f(-R * 0.78)} ${f(R * 0.3)},${f(-R * 0.34)} ${f(R * 0.3)},${f(R * 0.08)}Z`}
                    fill={accC}
                  />
                  <path d={`M${f(-R * 0.14)},${f(-R * 0.1)}C${f(-R * 0.08)},${f(-R * 0.5)} ${f(R * 0.1)},${f(-R * 0.82)} ${f(R * 0.36)},${f(-R * 1.02)}`} fill="none" stroke={hi} strokeWidth={f(R * 0.07)} strokeLinecap="round" opacity={0.7} />
                </g>,
                s,
              );
            })}
          </g>
        );
      }
      case "crown": {
        const w = R * 0.72;
        const h = R * 0.68;
        const gem = lighten(accC, 0.65);
        const x = d * 0.06;
        const y = topY - R * 0.42;
        return on(
          x,
          y,
          <g transform={`translate(${f(x)} ${f(y)}) rotate(-6)`}>
            <path
              d={`M${f(-w)},0L${f(-w)},${f(-0.62 * h)}L${f(-0.48 * w)},${f(-0.26 * h)}L0,${f(-h)}L${f(0.48 * w)},${f(-0.26 * h)}L${f(w)},${f(-0.62 * h)}L${f(w)},0Z`}
              fill={accC}
              stroke={accC}
              strokeWidth={f(R * 0.1)}
              strokeLinejoin="round"
            />
            <rect x={f(-w)} y={f(-R * 0.2)} width={f(2 * w)} height={f(R * 0.2)} fill={darken(accC, 0.18)} />
            <circle cx={f(-w)} cy={f(-0.66 * h)} r={f(R * 0.1)} fill={gem} />
            <circle cx={0} cy={f(-h - R * 0.04)} r={f(R * 0.12)} fill={gem} />
            <circle cx={f(w)} cy={f(-0.66 * h)} r={f(R * 0.1)} fill={gem} />
          </g>,
        );
      }
      case "antenna": {
        // The base rides the face; the tip trails it a little (the wobble).
        const baseY = topY - R * 0.3;
        const tipX = i.wob.x * 1.5;
        const tipY = topY - R * 1.2 + i.wob.y;
        const busy = fr.state === "thinking" || fr.state === "working";
        const pulse = busy && !i.reduced ? 0.5 + 0.5 * Math.sin(fr.t * 6) : 0.4;
        return (
          <g>
            <path d={`M${mp(0, baseY)}Q${mp(tipX * 0.3, (baseY + tipY) / 2)} ${mp(tipX, tipY)}`} fill="none" stroke={darken(accC, 0.3)} strokeWidth={f(R * 0.1)} strokeLinecap="round" />
            {on(0, baseY, <ellipse cy={f(baseY)} rx={f(R * 0.3)} ry={f(R * 0.1)} fill={darken(accC, 0.35)} />)}
            {on(
              tipX,
              tipY,
              <g>
                <circle cx={f(tipX)} cy={f(tipY)} r={f(R * 0.55)} fill={i.url("acc-glow")} opacity={0.3 + pulse * 0.5} />
                <circle cx={f(tipX)} cy={f(tipY)} r={f(R * 0.26)} fill={accC} />
                <circle cx={f(tipX - R * 0.08)} cy={f(tipY - R * 0.09)} r={f(R * 0.08)} fill="#FFFFFF" opacity={0.7} />
              </g>,
            )}
          </g>
        );
      }
      default:
        return null;
    }
  };

  // ---------- headphones ----------

  /** Cups beside the beans (as tall as most of a bean), a band over the
   * top with square shoulders, so it clears raised brows. The band's points
   * are each carried by the face, so it bends with the turn. */
  const cupX = d + hx + R * 0.8;
  const cupY = hy * 0.08;
  const cupW = R * 0.32;
  const cupH = hy * 0.72;
  const band = (): ReactElement | null => {
    const y0 = cupY - cupH * 0.8;
    const apex = topY - R * 1.3;
    const yc = (apex - 0.25 * y0) / 0.75;
    return (
      <path
        d={`M${mp(-cupX, y0)}C${mp(-cupX - R * 0.3, yc)} ${mp(cupX + R * 0.3, yc)} ${mp(cupX, y0)}`}
        fill="none"
        stroke={accC}
        strokeWidth={f(R * 0.2)}
        strokeLinecap="round"
      />
    );
  };
  const cups = (): ReactElement => (
    <g>
      {[-1, 1].map((s) =>
        on(
          s * cupX,
          cupY,
          <g>
            <rect x={f(s * cupX - cupW)} y={f(cupY - cupH)} width={f(2 * cupW)} height={f(2 * cupH)} rx={f(cupW)} fill={accC} />
            {tiny ? null : (
              <rect
                x={f(s * cupX + (s > 0 ? -cupW : cupW * 0.3))}
                y={f(cupY - cupH * 0.78)}
                width={f(cupW * 0.7)}
                height={f(cupH * 1.56)}
                rx={f(cupW * 0.35)}
                fill={darken(accC, 0.45)}
              />
            )}
          </g>,
          s,
        ),
      )}
    </g>
  );

  // ---------- layers ----------

  const topEl = det > 0.01 ? top() : null;
  const bandEl = acc === "headphones" && det > 0.01 ? band() : null;
  const back = (
    <>
      {bandEl ? <g opacity={det}>{bandEl}</g> : null}
      {topEl ? <g opacity={det}>{topEl}</g> : null}
      {cheeks()}
    </>
  );
  let front: ReactElement | null = null;
  if (acc === "glasses" || acc === "round_glasses") front = glasses();
  else if (acc === "sunglasses") front = sunglasses();
  else if (acc === "headphones") front = cups();
  return { back, brows: brows(), front };
}
