// "Surprise me": a random face that still looks designed. One hue drives
// the iris, the accent and the accessory; the whites stay near white; the
// odd shapes (stars, hearts) are rare.

import { hsl } from "./color";
import { ACCESSORIES, BROW_STYLES, IDLE_EMOTES, clampFace, type FaceConfig } from "./schema";

type Rand = () => number;

function pick<T>(rand: Rand, items: readonly T[]): T {
  return items[Math.floor(rand() * items.length)] ?? items[0]!;
}

function weighted<T>(rand: Rand, items: [T, number][]): T {
  const total = items.reduce((s, [, w]) => s + w, 0);
  let x = rand() * total;
  for (const [v, w] of items) {
    x -= w;
    if (x <= 0) return v;
  }
  return items[0]![0];
}

const range = (rand: Rand, lo: number, hi: number) => Math.round((lo + rand() * (hi - lo)) * 100) / 100;

export function randomFace(rand: Rand = Math.random): FaceConfig {
  const hue = rand() * 360;
  const flat = rand() < 0.15;
  const shape = flat
    ? weighted(rand, [["oval", 3], ["round", 2], ["line", 2]] as [FaceConfig["eyes"]["shape"], number][])
    : weighted(rand, [
        ["round", 4],
        ["oval", 3],
        ["almond", 3],
        ["droopy", 2],
        ["cat", 2],
        ["dot", 1],
        ["star", 0.4],
        ["heart", 0.4],
      ] as [FaceConfig["eyes"]["shape"], number][]);
  const accessory = rand() < 0.45 ? "none" : pick(rand, ACCESSORIES.filter((a) => a !== "none"));
  const accHue = hue + (rand() < 0.5 ? 150 : 35);
  const emotes = IDLE_EMOTES.filter(() => rand() < 0.5);

  return clampFace({
    v: 1,
    eyes: {
      shape,
      size: range(rand, 0.5, 0.72),
      spacing: range(rand, 0.4, 0.6),
      tilt: range(rand, -0.2, 0.3),
      sclera: flat ? hsl(hue, 85, 72) : hsl(hue, 35, 96),
      iris: { color: hsl(hue, range(rand, 65, 85), range(rand, 48, 62)), size: flat || shape === "dot" ? 0 : range(rand, 0.55, 0.75) },
      pupil: {
        color: hsl(hue, 40, 7),
        size: flat || shape === "dot" ? 0 : range(rand, 0.4, 0.6),
        shape: weighted(rand, [["round", 7], ["slit", 1], ["star", 0.7], ["heart", 0.5], ["ring", 0.7]] as [FaceConfig["eyes"]["pupil"]["shape"], number][]),
      },
      shine: range(rand, 0.5, 1),
      lids: { color: "#000000", rest: range(rand, 0, 0.3) },
      lashes: weighted(rand, [["none", 3], ["light", 2], ["full", 1.5]] as [FaceConfig["eyes"]["lashes"], number][]),
      glow: flat ? range(rand, 0.6, 0.9) : range(rand, 0, 0.35),
    },
    brows: { style: pick(rand, BROW_STYLES), color: hsl(hue + 20, 30, 86) },
    mouth: {
      style: weighted(rand, [["smile", 3], ["cat", 2], ["line", 2], ["dot", 1], ["open", 1], ["none", 1]] as [FaceConfig["mouth"]["style"], number][]),
      color: hsl(340 + rand() * 30, 75, 78),
    },
    cheeks: { blush: range(rand, 0, 0.6), color: hsl(345, 85, 70) },
    accessory,
    accessory_color: hsl(accHue, 80, 66),
    palette: { accent: hsl(hue + 30, 85, 70) },
    motion: {
      blink: range(rand, 0.2, 0.8),
      curiosity: range(rand, 0.3, 1),
      liveliness: range(rand, 0.2, 0.9),
      sleepy_after_min: pick(rand, [5, 8, 10, 12, 15, 20]),
    },
    idle_emotes: emotes.length ? emotes : ["look_around"],
  });
}
