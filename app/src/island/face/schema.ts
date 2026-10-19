// FaceConfig v1: the look of one synth on the island. The synth is the
// source of truth; the app caches it per synth_id. The same rules live in
// the synth (Go) and here: unknown keys are an error, colours are #RRGGBB,
// numbers out of range are an error (validateFace) or get clamped for
// drawing (clampFace), enums only from the lists below, at most 6 idle
// emotes, the whole JSON at most 4 KB. A patch is a deep merge of objects;
// arrays are replaced whole.
//
// No imports beyond presets.json so `node --test` runs this file as is.

import presetsFile from "./presets.json" with { type: "json" };

export const EYE_SHAPES = ["round", "oval", "almond", "droopy", "cat", "dot", "line", "star", "heart"] as const;
export const PUPIL_SHAPES = ["round", "slit", "star", "heart", "ring"] as const;
export const LASHES = ["none", "light", "full"] as const;
export const BROW_STYLES = ["none", "thin", "thick", "arched", "flat", "worried"] as const;
export const MOUTH_STYLES = ["none", "line", "smile", "cat", "dot", "open"] as const;
export const ACCESSORIES = [
  "none",
  "glasses",
  "round_glasses",
  "sunglasses",
  "cat_ears",
  "bow",
  "halo",
  "horns",
  "headphones",
  "crown",
  "antenna",
] as const;
export const IDLE_EMOTES = ["wink", "look_around", "yawn", "heart_eyes", "squint", "surprised"] as const;
export const FACE_STATES = [
  "idle",
  "sleeping",
  "listening",
  "accepted",
  "thinking",
  "working",
  "driving",
  "approval",
  "done",
  "error",
  "offline",
] as const;

export type EyeShape = (typeof EYE_SHAPES)[number];
export type PupilShape = (typeof PUPIL_SHAPES)[number];
export type Lashes = (typeof LASHES)[number];
export type BrowStyle = (typeof BROW_STYLES)[number];
export type MouthStyle = (typeof MOUTH_STYLES)[number];
export type Accessory = (typeof ACCESSORIES)[number];
export type IdleEmote = (typeof IDLE_EMOTES)[number];
export type FaceState = (typeof FACE_STATES)[number];

export interface FaceConfig {
  v: 1;
  preset?: string;
  eyes: {
    shape: EyeShape;
    size: number;
    spacing: number;
    tilt: number;
    sclera: string;
    iris: { color: string; size: number };
    pupil: { color: string; size: number; shape: PupilShape };
    shine: number;
    lids: { color: string; rest: number };
    lashes: Lashes;
    glow: number;
  };
  brows: { style: BrowStyle; color: string };
  mouth: { style: MouthStyle; color: string };
  cheeks: { blush: number; color: string };
  accessory: Accessory;
  accessory_color: string;
  palette: { accent: string };
  motion: { blink: number; curiosity: number; liveliness: number; sleepy_after_min: number };
  idle_emotes: IdleEmote[];
}

export interface FacePreset {
  name: string;
  title_en: string;
  title_ru: string;
  blurb_en: string;
  blurb_ru: string;
  face: FaceConfig;
}

export const MAX_FACE_BYTES = 4096;
export const MAX_IDLE_EMOTES = 6;
export const MAX_SLEEPY_MIN = 120;

// ---------- the shape of the contract, as data ----------

type Leaf =
  | { kind: "num"; min: number; max: number }
  | { kind: "color" }
  | { kind: "enum"; values: readonly string[] }
  | { kind: "preset" }
  | { kind: "version" }
  | { kind: "emotes" };
type Node = Leaf | { kind: "obj"; keys: Record<string, Node>; optional?: readonly string[] };

const unit: Leaf = { kind: "num", min: 0, max: 1 };
const color: Leaf = { kind: "color" };
const en = (values: readonly string[]): Leaf => ({ kind: "enum", values });
const obj = (keys: Record<string, Node>, optional?: readonly string[]): Node => ({ kind: "obj", keys, optional });

const SPEC: Node = obj(
  {
    v: { kind: "version" },
    preset: { kind: "preset" },
    eyes: obj({
      shape: en(EYE_SHAPES),
      size: unit,
      spacing: unit,
      tilt: { kind: "num", min: -1, max: 1 },
      sclera: color,
      iris: obj({ color, size: unit }),
      pupil: obj({ color, size: unit, shape: en(PUPIL_SHAPES) }),
      shine: unit,
      lids: obj({ color, rest: unit }),
      lashes: en(LASHES),
      glow: unit,
    }),
    brows: obj({ style: en(BROW_STYLES), color }),
    mouth: obj({ style: en(MOUTH_STYLES), color }),
    cheeks: obj({ blush: unit, color }),
    accessory: en(ACCESSORIES),
    accessory_color: color,
    palette: obj({ accent: color }),
    motion: obj({
      blink: unit,
      curiosity: unit,
      liveliness: unit,
      sleepy_after_min: { kind: "num", min: 0, max: MAX_SLEEPY_MIN },
    }),
    idle_emotes: { kind: "emotes" },
  },
  ["preset"],
);

const COLOR_RE = /^#[0-9A-Fa-f]{6}$/;
const PRESET_RE = /^[a-z0-9_]{1,32}$/;

function isPlainObject(x: unknown): x is Record<string, unknown> {
  return typeof x === "object" && x !== null && !Array.isArray(x);
}

function byteLength(x: unknown): number {
  const json = JSON.stringify(x);
  return json === undefined ? 0 : new TextEncoder().encode(json).length;
}

/** Returns a reason string, or null when the leaf value is fine. */
function checkLeaf(leaf: Leaf, value: unknown, path: string): string | null {
  switch (leaf.kind) {
    case "num":
      if (typeof value !== "number" || !Number.isFinite(value)) return `${path}: must be a number`;
      if (value < leaf.min || value > leaf.max) return `${path}: must be between ${leaf.min} and ${leaf.max}`;
      return null;
    case "color":
      return typeof value === "string" && COLOR_RE.test(value) ? null : `${path}: must be a colour like #A1B2C3`;
    case "enum":
      return typeof value === "string" && leaf.values.includes(value)
        ? null
        : `${path}: must be one of ${leaf.values.join(", ")}`;
    case "preset":
      return typeof value === "string" && PRESET_RE.test(value) ? null : `${path}: must be a preset name (a-z, 0-9, _)`;
    case "version":
      return value === 1 ? null : `${path}: must be 1`;
    case "emotes": {
      if (!Array.isArray(value)) return `${path}: must be a list`;
      if (value.length > MAX_IDLE_EMOTES) return `${path}: at most ${MAX_IDLE_EMOTES} emotes`;
      const seen = new Set<string>();
      for (const item of value) {
        if (typeof item !== "string" || !(IDLE_EMOTES as readonly string[]).includes(item))
          return `${path}: each must be one of ${IDLE_EMOTES.join(", ")}`;
        if (seen.has(item)) return `${path}: "${item}" is listed twice`;
        seen.add(item);
      }
      return null;
    }
  }
}

function checkNode(node: Node, value: unknown, path: string, partial: boolean): string | null {
  if (node.kind !== "obj") return checkLeaf(node, value, path);
  if (!isPlainObject(value)) return `${path || "face"}: must be an object`;
  for (const key of Object.keys(value)) {
    if (!(key in node.keys)) return `${path ? path + "." : ""}${key}: unknown key`;
  }
  for (const [key, child] of Object.entries(node.keys)) {
    const p = path ? `${path}.${key}` : key;
    if (!(key in value)) {
      if (partial || node.optional?.includes(key)) continue;
      return `${p}: missing`;
    }
    const problem = checkNode(child, value[key], p, partial);
    if (problem) return problem;
  }
  return null;
}

export type FaceResult = { ok: true; face: FaceConfig } | { ok: false; reason: string };

/** Strict check of a full FaceConfig (what the synth stores and sends). */
export function validateFace(input: unknown): FaceResult {
  const size = byteLength(input);
  if (size > MAX_FACE_BYTES) return { ok: false, reason: `face: ${size} bytes, at most ${MAX_FACE_BYTES}` };
  const problem = checkNode(SPEC, input, "", false);
  if (problem) return { ok: false, reason: problem };
  return { ok: true, face: input as FaceConfig };
}

/** Strict check of a patch: the same rules, but every key may be missing. */
export function validatePatch(input: unknown): { ok: true } | { ok: false; reason: string } {
  const size = byteLength(input);
  if (size > MAX_FACE_BYTES) return { ok: false, reason: `patch: ${size} bytes, at most ${MAX_FACE_BYTES}` };
  const problem = checkNode(SPEC, input, "", true);
  if (problem) return { ok: false, reason: problem };
  return { ok: true };
}

/** Deep merge: objects merge key by key, everything else (arrays too) is replaced. */
export function deepMerge<T>(base: T, patch: unknown): T {
  if (!isPlainObject(base) || !isPlainObject(patch)) return (patch === undefined ? base : patch) as T;
  const out: Record<string, unknown> = { ...base };
  for (const [key, value] of Object.entries(patch)) {
    out[key] = key in out ? deepMerge(out[key], value) : value;
  }
  return out as T;
}

/** Applies a patch to a face and checks the result. */
export function mergeFace(base: FaceConfig, patch: unknown): FaceResult {
  const p = validatePatch(patch);
  if (!p.ok) return p;
  return validateFace(deepMerge(base, patch));
}

function clampNode(node: Node, value: unknown, fallback: unknown): unknown {
  if (node.kind === "obj") {
    const src = isPlainObject(value) ? value : {};
    const fb = isPlainObject(fallback) ? fallback : {};
    const out: Record<string, unknown> = {};
    for (const [key, child] of Object.entries(node.keys)) {
      const v = clampNode(child, src[key], fb[key]);
      if (v !== undefined) out[key] = v;
    }
    return out;
  }
  switch (node.kind) {
    case "num":
      if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
      return Math.min(node.max, Math.max(node.min, value));
    case "color":
      return typeof value === "string" && COLOR_RE.test(value) ? value.toUpperCase() : fallback;
    case "enum":
      return typeof value === "string" && node.values.includes(value) ? value : fallback;
    case "preset":
      return typeof value === "string" && PRESET_RE.test(value) ? value : undefined;
    case "version":
      return 1;
    case "emotes": {
      if (!Array.isArray(value)) return fallback;
      const out: string[] = [];
      for (const item of value) {
        if (typeof item === "string" && (IDLE_EMOTES as readonly string[]).includes(item) && !out.includes(item)) out.push(item);
      }
      return out.slice(0, MAX_IDLE_EMOTES);
    }
  }
}

/** Lenient: whatever arrives, returns a face that can be drawn. Missing or
 * broken parts come from `fallback` (the classic preset if not given),
 * numbers are clamped into range, unknown keys dropped. */
export function clampFace(input: unknown, fallback?: FaceConfig): FaceConfig {
  return clampNode(SPEC, input, fallback ?? presetFace("classic")) as FaceConfig;
}

// ---------- presets ----------

interface PresetsFile {
  v: 1;
  default_pool: string[];
  presets: FacePreset[];
}

const file = presetsFile as PresetsFile;

export const PRESETS: readonly FacePreset[] = file.presets;
export const DEFAULT_POOL: readonly string[] = file.default_pool;

export function findPreset(name: string | undefined): FacePreset | undefined {
  return name ? PRESETS.find((p) => p.name === name) : undefined;
}

/** A fresh copy of a preset's face (classic for an unknown name). */
export function presetFace(name: string): FaceConfig {
  const preset = findPreset(name) ?? PRESETS[0];
  if (!preset) throw new Error("presets.json is empty");
  return JSON.parse(JSON.stringify(preset.face)) as FaceConfig;
}

/** FNV-1a, 32 bit, over the UTF-8 bytes. The synth uses the same. */
export function fnv1a32(text: string): number {
  let hash = 0x811c9dc5;
  for (const byte of new TextEncoder().encode(text)) {
    hash ^= byte;
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash >>> 0;
}

/** The look a synth has before anyone chose one: a preset from
 * `default_pool` picked by FNV-1a(synth_id) mod pool size. */
export function defaultFaceFor(synthId: string): FaceConfig {
  const pool = DEFAULT_POOL.length ? DEFAULT_POOL : PRESETS.map((p) => p.name);
  const name = pool[fnv1a32(synthId) % pool.length] ?? "classic";
  return presetFace(name);
}

/** The face to draw for a synth: its own if any (clamped), else its default. */
export function faceFor(synthId: string, stored: unknown): FaceConfig {
  const fallback = defaultFaceFor(synthId);
  return stored == null ? fallback : clampFace(stored, fallback);
}
