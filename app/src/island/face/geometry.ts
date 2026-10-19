// Shapes of the face, as SVG path strings. Units are face units: the face
// is laid out around (0, 0) = the point between the eyes, y grows down. An
// eye's base radius R is 10..20 units (eyes.size). Each shape is drawn for
// the right eye with its outer corner toward +x; the left eye is mirrored.

import type { EyeShape } from "./schema";

/** A number for a path string: two decimals, no trailing zeros. */
export const f = (n: number) => (Math.round(n * 100) / 100).toString();

/** A star with softened corners: `points` tips, outer radius `ro`, inner `ri`. */
export function softStar(cx: number, cy: number, ro: number, ri: number, points = 5, round = 0.28, rot = -90): string {
  const verts: [number, number][] = [];
  for (let i = 0; i < points * 2; i++) {
    const r = i % 2 === 0 ? ro : ri;
    const a = ((rot + (i * 180) / points) * Math.PI) / 180;
    verts.push([cx + r * Math.cos(a), cy + r * Math.sin(a)]);
  }
  const n = verts.length;
  const lerp = (a: [number, number], b: [number, number], t: number): [number, number] => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
  let d = "";
  for (let i = 0; i < n; i++) {
    const prev = verts[(i - 1 + n) % n]!;
    const cur = verts[i]!;
    const next = verts[(i + 1) % n]!;
    const a = lerp(cur, prev, round);
    const b = lerp(cur, next, round);
    d += `${i === 0 ? "M" : "L"}${f(a[0])},${f(a[1])}Q${f(cur[0])},${f(cur[1])} ${f(b[0])},${f(b[1])}`;
  }
  return d + "Z";
}

/** A heart of about `r` half-size centred on (cx, cy). */
export function heart(cx: number, cy: number, r: number): string {
  const p = (x: number, y: number) => `${f(cx + x * r)},${f(cy + y * r)}`;
  return (
    `M${p(0, 0.95)}` +
    `C${p(-0.25, 0.75)} ${p(-1.05, 0.25)} ${p(-1.05, -0.3)}` +
    `C${p(-1.05, -0.8)} ${p(-0.45, -1.05)} ${p(0, -0.55)}` +
    `C${p(0.45, -1.05)} ${p(1.05, -0.8)} ${p(1.05, -0.3)}` +
    `C${p(1.05, 0.25)} ${p(0.25, 0.75)} ${p(0, 0.95)}Z`
  );
}

/** A four-pointed sparkle. */
export function sparkle(cx: number, cy: number, r: number): string {
  return softStar(cx, cy, r, r * 0.28, 4, 0.12, -90);
}

// ---------- eye outlines ----------

type Pt = [number, number];
type Seg = { k: "L"; p: [Pt, Pt] } | { k: "Q"; p: [Pt, Pt, Pt] } | { k: "C"; p: [Pt, Pt, Pt, Pt] };

export interface EyeGeom {
  /** Half extents of the eye: lids, gaze range and placement use them. */
  hx: number;
  hy: number;
  /** Outline, outer corner toward +x. */
  path: string;
  /** The outer corner (where a liner wing starts). */
  corner: Pt;
  /** Top and bottom of the outline at x (NaN outside the eye). */
  top(x: number): number;
  bottom(x: number): number;
}

function segsToPath(segs: Seg[]): string {
  let d = "";
  let at: Pt | null = null;
  for (const s of segs) {
    const p0 = s.p[0];
    if (!at || at[0] !== p0[0] || at[1] !== p0[1]) d += `M${f(p0[0])},${f(p0[1])}`;
    const rest = s.p.slice(1).map((q) => `${f(q[0])},${f(q[1])}`).join(" ");
    d += `${s.k}${rest}`;
    at = s.p[s.p.length - 1]!;
  }
  return d + "Z";
}

function evalSeg(s: Seg, t: number): Pt {
  const u = 1 - t;
  if (s.k === "L") return [s.p[0][0] * u + s.p[1][0] * t, s.p[0][1] * u + s.p[1][1] * t];
  if (s.k === "Q") {
    const [a, b, c] = s.p;
    return [u * u * a[0] + 2 * u * t * b[0] + t * t * c[0], u * u * a[1] + 2 * u * t * b[1] + t * t * c[1]];
  }
  const [a, b, c, d] = s.p;
  const w0 = u * u * u;
  const w1 = 3 * u * u * t;
  const w2 = 3 * u * t * t;
  const w3 = t * t * t;
  return [w0 * a[0] + w1 * b[0] + w2 * c[0] + w3 * d[0], w0 * a[1] + w1 * b[1] + w2 * c[1] + w3 * d[1]];
}

const BINS = 64;

function build(segs: Seg[], hx: number, hy: number): EyeGeom {
  const pts: Pt[] = [];
  for (const s of segs) for (let i = 0; i < 24; i++) pts.push(evalSeg(s, i / 24));
  let minX = Infinity;
  let maxX = -Infinity;
  let corner: Pt = [hx, 0];
  for (const p of pts) {
    if (p[0] < minX) minX = p[0];
    if (p[0] > maxX) {
      maxX = p[0];
      corner = p;
    }
  }
  const tops: number[] = [];
  const bottoms: number[] = [];
  for (let b = 0; b <= BINS; b++) {
    const x = minX + ((maxX - minX) * b) / BINS;
    let lo = Infinity;
    let hi = -Infinity;
    for (let i = 0; i < pts.length; i++) {
      const p = pts[i]!;
      const q = pts[(i + 1) % pts.length]!;
      if ((p[0] - x) * (q[0] - x) > 0 || p[0] === q[0]) continue;
      const y = p[1] + ((q[1] - p[1]) * (x - p[0])) / (q[0] - p[0]);
      lo = Math.min(lo, y);
      hi = Math.max(hi, y);
    }
    tops.push(Number.isFinite(lo) ? lo : 0);
    bottoms.push(Number.isFinite(hi) ? hi : 0);
  }
  const look = (arr: number[]) => (x: number) => {
    if (x < minX || x > maxX) return NaN;
    const u = ((x - minX) / (maxX - minX)) * BINS;
    const i = Math.min(BINS - 1, Math.floor(u));
    const fr = u - i;
    return arr[i]! * (1 - fr) + arr[i + 1]! * fr;
  };
  return { hx, hy, path: segsToPath(segs), corner, top: look(tops), bottom: look(bottoms) };
}

const K = 0.5523;

function ellipseSegs(rx: number, ry: number): Seg[] {
  return [
    { k: "C", p: [[rx, 0], [rx, ry * K], [rx * K, ry], [0, ry]] },
    { k: "C", p: [[0, ry], [-rx * K, ry], [-rx, ry * K], [-rx, 0]] },
    { k: "C", p: [[-rx, 0], [-rx, -ry * K], [-rx * K, -ry], [0, -ry]] },
    { k: "C", p: [[0, -ry], [rx * K, -ry], [rx, -ry * K], [rx, 0]] },
  ];
}

/** Closed outline through cubic pieces given as [start, c1, c2, end] in units of hx, hy. */
function cubics(hx: number, hy: number, pieces: [number, number][][]): Seg[] {
  return pieces.map((pc) => ({ k: "C", p: pc.map(([x, y]) => [x * hx, y * hy] as Pt) as [Pt, Pt, Pt, Pt] }));
}

const cache = new Map<string, EyeGeom>();

export function eyeGeom(shape: EyeShape, R: number): EyeGeom {
  const key = `${shape}:${R.toFixed(2)}`;
  const hit = cache.get(key);
  if (hit) return hit;
  const g = makeEye(shape, R);
  if (cache.size > 200) cache.clear();
  cache.set(key, g);
  return g;
}

function makeEye(shape: EyeShape, R: number): EyeGeom {
  switch (shape) {
    case "round":
      return build(ellipseSegs(R, R), R, R);
    case "oval":
      return build(ellipseSegs(R * 0.8, R * 1.12), R * 0.8, R * 1.12);
    case "almond": {
      const hx = R * 1.18;
      const hy = R * 0.78;
      return build(
        cubics(hx, hy, [
          [[-1, 0.12], [-0.82, -0.75], [0.35, -1.32], [1, -0.18]],
          [[1, -0.18], [0.72, 0.95], [-0.55, 1.32], [-1, 0.12]],
        ]),
        hx,
        hy,
      );
    }
    case "cat": {
      const hx = R * 1.12;
      const hy = R * 0.86;
      return build(
        cubics(hx, hy, [
          [[-0.98, 0.2], [-0.85, -0.85], [0.25, -1.3], [1.06, -0.52]],
          [[1.06, -0.52], [0.9, 0.7], [0.1, 1.22], [-0.42, 1.05]],
          [[-0.42, 1.05], [-0.75, 0.92], [-0.98, 0.6], [-0.98, 0.2]],
        ]),
        hx,
        hy,
      );
    }
    case "droopy": {
      const hx = R * 1.04;
      const hy = R * 0.9;
      return build(
        cubics(hx, hy, [
          [[-0.96, -0.45], [-0.7, -1.2], [0.55, -0.95], [1, 0.12]],
          [[1, 0.12], [1.08, 0.75], [0.55, 1.08], [0, 1.08]],
          [[0, 1.08], [-0.62, 1.08], [-1.08, 0.55], [-0.96, -0.45]],
        ]),
        hx,
        hy,
      );
    }
    case "dot": {
      const r = R * 0.6;
      return build(ellipseSegs(r, r), r, r);
    }
    case "line": {
      const hx = R * 1.15;
      const hy = R * 0.4;
      const a = hx - hy;
      const k = hy * K;
      return build(
        [
          { k: "L", p: [[-a, -hy], [a, -hy]] },
          { k: "C", p: [[a, -hy], [a + k, -hy], [hx, -k], [hx, 0]] },
          { k: "C", p: [[hx, 0], [hx, k], [a + k, hy], [a, hy]] },
          { k: "L", p: [[a, hy], [-a, hy]] },
          { k: "C", p: [[-a, hy], [-a - k, hy], [-hx, k], [-hx, 0]] },
          { k: "C", p: [[-hx, 0], [-hx, -k], [-a - k, -hy], [-a, -hy]] },
        ],
        hx,
        hy,
      );
    }
    case "star":
      return build(starSegs(0, R * 0.08, R * 1.18, R * 0.62, 5, 0.3), R * 1.05, R * 1.05);
    case "heart": {
      const r = R * 1.02;
      const cy = R * 0.05;
      return build(
        cubics(r, r, [
          [[0, 0.95], [-0.25, 0.75], [-1.05, 0.25], [-1.05, -0.3]],
          [[-1.05, -0.3], [-1.05, -0.8], [-0.45, -1.05], [0, -0.55]],
          [[0, -0.55], [0.45, -1.05], [1.05, -0.8], [1.05, -0.3]],
          [[1.05, -0.3], [1.05, 0.25], [0.25, 0.75], [0, 0.95]],
        ]).map((s) => ({ ...s, p: s.p.map(([x, y]) => [x, y + cy]) as typeof s.p }) as Seg),
        R * 1.05,
        R * 0.98,
      );
    }
  }
}

function starSegs(cx: number, cy: number, ro: number, ri: number, points: number, round: number): Seg[] {
  const verts: Pt[] = [];
  for (let i = 0; i < points * 2; i++) {
    const r = i % 2 === 0 ? ro : ri;
    const a = ((-90 + (i * 180) / points) * Math.PI) / 180;
    verts.push([cx + r * Math.cos(a), cy + r * Math.sin(a)]);
  }
  const n = verts.length;
  const lerpP = (a: Pt, b: Pt, t: number): Pt => [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
  const segs: Seg[] = [];
  for (let i = 0; i < n; i++) {
    const prev = verts[(i - 1 + n) % n]!;
    const cur = verts[i]!;
    const next = verts[(i + 1) % n]!;
    const a = lerpP(cur, prev, round);
    const b = lerpP(cur, next, round);
    const nextA = lerpP(next, cur, round);
    segs.push({ k: "Q", p: [a, cur, b] });
    segs.push({ k: "L", p: [b, nextA] });
  }
  return segs;
}

