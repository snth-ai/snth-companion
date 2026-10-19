// Small colour helpers for drawing on the black island. Colours come in as
// #RRGGBB (the schema guarantees it after clampFace).

export type RGB = [number, number, number];

export function hexToRgb(hex: string): RGB {
  const n = parseInt(hex.slice(1, 7), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function rgbToHex([r, g, b]: RGB): string {
  const h = (v: number) => Math.round(Math.min(255, Math.max(0, v))).toString(16).padStart(2, "0");
  return `#${h(r)}${h(g)}${h(b)}`;
}

export function mix(a: string, b: string, t: number): string {
  const x = hexToRgb(a);
  const y = hexToRgb(b);
  return rgbToHex([x[0] + (y[0] - x[0]) * t, x[1] + (y[1] - x[1]) * t, x[2] + (y[2] - x[2]) * t]);
}

export const lighten = (c: string, t: number) => mix(c, "#FFFFFF", t);
export const darken = (c: string, t: number) => mix(c, "#000000", t);

/** Relative luminance, 0 (black) .. 1 (white). */
export function luminance(hex: string): number {
  const lin = (v: number) => {
    const s = v / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  const [r, g, b] = hexToRgb(hex);
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

/** Line art (brows, mouth, accessories) must show on black: a colour darker
 * than `min` luminance is lifted toward white until it reads. */
export function visibleOnBlack(hex: string, min = 0.12): string {
  if (luminance(hex) >= min) return hex;
  let lo = 0;
  let hi = 1;
  for (let i = 0; i < 12; i++) {
    const mid = (lo + hi) / 2;
    if (luminance(lighten(hex, mid)) >= min) hi = mid;
    else lo = mid;
  }
  return lighten(hex, hi);
}

/** The offline look: drained of colour and dimmer. `t` 0..1. */
export function drain(hex: string, t: number): string {
  if (t <= 0.001) return hex;
  const [r, g, b] = hexToRgb(hex);
  const y = 0.3 * r + 0.59 * g + 0.11 * b;
  const grey = rgbToHex([y, y, y]);
  return darken(mix(hex, grey, 0.8 * t), 0.5 * t);
}
