// Schema tests. Run from app/: node --test src/island/face/
// (Node 22.6+ strips the TypeScript types of schema.ts by itself.)

import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import {
  ACCESSORIES,
  DEFAULT_POOL,
  EYE_SHAPES,
  IDLE_EMOTES,
  MAX_FACE_BYTES,
  PRESETS,
  clampFace,
  deepMerge,
  defaultFaceFor,
  faceFor,
  fnv1a32,
  mergeFace,
  presetFace,
  validateFace,
  validatePatch,
} from "./schema.ts";

const classic = () => presetFace("classic");
const bad = (face, pattern) => {
  const r = validateFace(face);
  assert.equal(r.ok, false, `expected an error matching ${pattern}`);
  assert.match(r.reason, pattern);
};

test("every preset is a valid face with titles in both languages", () => {
  const names = new Set();
  for (const p of PRESETS) {
    const r = validateFace(p.face);
    assert.ok(r.ok, `${p.name}: ${r.ok ? "" : r.reason}`);
    assert.equal(p.face.preset, p.name);
    for (const k of ["title_en", "title_ru", "blurb_en", "blurb_ru"]) assert.ok(p[k].trim(), `${p.name}.${k}`);
    assert.ok(!names.has(p.name), `duplicate ${p.name}`);
    names.add(p.name);
  }
  for (const n of ["classic", "mia", "sleepy", "cat", "robot", "star", "shy", "cool"]) assert.ok(names.has(n), n);
  assert.ok(PRESETS.length >= 8);
  for (const n of DEFAULT_POOL) assert.ok(names.has(n), `pool: ${n}`);
});

test("presets have distinct characters, not recolours", () => {
  const sig = (f) => [f.eyes.shape, f.eyes.pupil.shape, f.brows.style, f.mouth.style, f.accessory, f.eyes.lashes].join("/");
  assert.equal(new Set(PRESETS.map((p) => sig(p.face))).size, PRESETS.length);
});

test("no em-dash anywhere in the presets file", () => {
  const raw = readFileSync(new URL("./presets.json", import.meta.url), "utf8");
  assert.ok(!raw.includes(String.fromCharCode(0x2014)));
});

test("unknown keys are an error, top level and nested", () => {
  bad({ ...classic(), extra: 1 }, /^extra: unknown key/);
  const f = classic();
  f.eyes.iris.hue = 3;
  bad(f, /^eyes\.iris\.hue: unknown key/);
});

test("missing keys are an error, except preset", () => {
  const f = classic();
  delete f.mouth;
  bad(f, /^mouth: missing/);
  const g = classic();
  delete g.preset;
  assert.ok(validateFace(g).ok);
});

test("version must be 1", () => {
  bad({ ...classic(), v: 2 }, /^v: must be 1/);
});

test("colours are #RRGGBB", () => {
  for (const c of ["#FFF", "red", "#GG0000", "#12345678", 123]) {
    const f = classic();
    f.eyes.sclera = c;
    bad(f, /^eyes\.sclera: must be a colour/);
  }
  const f = classic();
  f.eyes.sclera = "#a1b2c3";
  assert.ok(validateFace(f).ok);
});

test("numbers out of range are an error", () => {
  const f = classic();
  f.eyes.size = 1.01;
  bad(f, /^eyes\.size: must be between 0 and 1/);
  const g = classic();
  g.eyes.tilt = -1.5;
  bad(g, /^eyes\.tilt: must be between -1 and 1/);
  const h = classic();
  h.motion.sleepy_after_min = 121;
  bad(h, /sleepy_after_min: must be between 0 and 120/);
  const i = classic();
  i.eyes.glow = "0.5";
  bad(i, /^eyes\.glow: must be a number/);
  const j = classic();
  j.eyes.tilt = -1;
  j.motion.sleepy_after_min = 0;
  assert.ok(validateFace(j).ok);
});

test("enums only from the list", () => {
  const f = classic();
  f.eyes.shape = "square";
  bad(f, /^eyes\.shape: must be one of round/);
  const g = classic();
  g.accessory = "hat";
  bad(g, /^accessory: must be one of/);
  for (const a of ACCESSORIES) assert.ok(validateFace({ ...classic(), accessory: a }).ok, a);
});

test("pill eyes: the newest shape, last in the list, valid with iris and pupil kept", () => {
  assert.equal(EYE_SHAPES[EYE_SHAPES.length - 1], "pill");
  const f = classic();
  f.eyes.shape = "pill";
  assert.ok(validateFace(f).ok);
  assert.ok(validatePatch({ eyes: { shape: "pill" } }).ok);
  assert.equal(mergeFace(classic(), { eyes: { shape: "pill" } }).face.eyes.iris.color, classic().eyes.iris.color);
  assert.equal(clampFace(f).eyes.shape, "pill");
});

test("buddy: white pill eyes and a smile, valid, under 4 KB, not handed out by default", () => {
  const b = presetFace("buddy");
  const r = validateFace(b);
  assert.ok(r.ok, r.ok ? "" : r.reason);
  assert.equal(b.eyes.shape, "pill");
  assert.equal(b.mouth.style, "smile");
  assert.equal(b.brows.style, "none");
  assert.equal(b.accessory, "none");
  assert.equal(b.cheeks.blush, 0);
  assert.ok(new TextEncoder().encode(JSON.stringify(b)).length <= MAX_FACE_BYTES);
  assert.ok(!DEFAULT_POOL.includes("buddy"));
  for (let i = 0; i < 300; i++) assert.notEqual(defaultFaceFor(`synth_${i}`).preset, "buddy");
});

test("idle emotes: at most 6, from the list, no repeats", () => {
  assert.ok(validateFace({ ...classic(), idle_emotes: [...IDLE_EMOTES] }).ok);
  assert.ok(validateFace({ ...classic(), idle_emotes: [] }).ok);
  bad({ ...classic(), idle_emotes: [...IDLE_EMOTES, "wink"] }, /at most 6/);
  bad({ ...classic(), idle_emotes: ["dance"] }, /each must be one of/);
  bad({ ...classic(), idle_emotes: ["wink", "wink"] }, /listed twice/);
  bad({ ...classic(), idle_emotes: "wink" }, /must be a list/);
});

test("the whole JSON is at most 4 KB", () => {
  const huge = { ...classic(), preset: "x".repeat(MAX_FACE_BYTES) };
  bad(huge, /bytes, at most 4096/);
  assert.equal(validatePatch({ preset: "y".repeat(MAX_FACE_BYTES) }).ok, false);
});

test("patch: deep merge of objects, arrays replaced whole", () => {
  const base = classic();
  const r = mergeFace(base, { eyes: { iris: { color: "#112233" } }, idle_emotes: ["yawn"] });
  assert.ok(r.ok);
  assert.equal(r.face.eyes.iris.color, "#112233");
  assert.equal(r.face.eyes.iris.size, base.eyes.iris.size);
  assert.equal(r.face.eyes.shape, base.eyes.shape);
  assert.deepEqual(r.face.idle_emotes, ["yawn"]);
  assert.equal(base.eyes.iris.color, "#3D7BFF", "base untouched");
});

test("patch errors keep the strict rules", () => {
  assert.match(mergeFace(classic(), { eyes: { colour: "#000000" } }).reason, /eyes\.colour: unknown key/);
  assert.match(mergeFace(classic(), { eyes: { size: 2 } }).reason, /eyes\.size: must be between/);
  assert.match(mergeFace(classic(), { eyes: "big" }).reason, /eyes: must be an object/);
  assert.equal(mergeFace(classic(), {}).ok, true);
});

test("deepMerge leaves the inputs alone", () => {
  const a = { x: { y: 1, z: [1, 2] } };
  const out = deepMerge(a, { x: { z: [3] } });
  assert.deepEqual(out, { x: { y: 1, z: [3] } });
  assert.deepEqual(a, { x: { y: 1, z: [1, 2] } });
});

test("clampFace: always drawable, numbers clamped, junk replaced", () => {
  const f = classic();
  f.eyes.size = 9;
  f.eyes.tilt = -3;
  f.eyes.sclera = "white";
  f.eyes.shape = "square";
  f.idle_emotes = ["wink", "wink", "dance", "yawn"];
  f.extra = true;
  f.motion.blink = Number.NaN;
  const c = clampFace(f);
  assert.ok(validateFace(c).ok, validateFace(c).reason);
  assert.equal(c.eyes.size, 1);
  assert.equal(c.eyes.tilt, -1);
  assert.equal(c.eyes.sclera, classic().eyes.sclera);
  assert.equal(c.eyes.shape, "round");
  assert.deepEqual(c.idle_emotes, ["wink", "yawn"]);
  assert.equal("extra" in c, false);
  assert.equal(c.motion.blink, classic().motion.blink);
  for (const junk of [null, undefined, 42, "face", [], {}]) assert.ok(validateFace(clampFace(junk)).ok);
});

test("FNV-1a 32 matches the reference values", () => {
  assert.equal(fnv1a32(""), 0x811c9dc5);
  assert.equal(fnv1a32("a"), 0xe40c292c);
  assert.equal(fnv1a32("foobar"), 0xbf9cf968);
});

test("defaultFaceFor is deterministic, from the pool, and varied", () => {
  assert.deepEqual(defaultFaceFor("synth-abc"), defaultFaceFor("synth-abc"));
  const seen = new Set();
  for (let i = 0; i < 200; i++) {
    const f = defaultFaceFor(`synth_${i}`);
    assert.ok(DEFAULT_POOL.includes(f.preset));
    assert.ok(validateFace(f).ok);
    seen.add(f.preset);
  }
  assert.equal(seen.size, DEFAULT_POOL.length);
  const name = DEFAULT_POOL[fnv1a32("mia") % DEFAULT_POOL.length];
  assert.equal(defaultFaceFor("mia").preset, name);
});

test("faceFor: stored face wins, broken parts fall back to the synth's default", () => {
  assert.equal(faceFor("x", null).preset, defaultFaceFor("x").preset);
  const stored = { eyes: { iris: { color: "#00FF00" } } };
  const f = faceFor("x", stored);
  assert.equal(f.eyes.iris.color, "#00FF00");
  assert.equal(f.eyes.shape, defaultFaceFor("x").eyes.shape);
});

test("presetFace returns a copy", () => {
  const a = presetFace("cat");
  a.eyes.size = 0;
  assert.notEqual(presetFace("cat").eyes.size, 0);
});
