// Gaze rig tests. Run from app/: node --test src/island/face/*.test.mjs

import assert from "node:assert/strict";
import { test } from "node:test";
import { styleOf } from "./motion.ts";
import { GazeRig, MICRO, SACCADE, onSphere, placePill, saccadeCurve } from "./rig.ts";
import { presetFace } from "./schema.ts";

const geom = { d: 37.8, hx: 13.3, hy: 23.1, mouthX: 0, mouthY: 19.9 };

/** Steps a rig at 120 Hz from t0 for `secs`, returns the eye track. */
function run(rig, t0, secs, input) {
  const track = [];
  let t = t0;
  for (let i = 0; i < Math.round(secs * 120); i++) {
    t += 1 / 120;
    const out = rig.step(t, 1 / 120, typeof input === "function" ? input(t) : input);
    track.push({ t, x: out.eye.x, y: out.eye.y, blink: out.blink });
  }
  return track;
}

test("saccade curve: starts at 0, overshoots at the end of the jump, settles to 1", () => {
  assert.equal(saccadeCurve(0, 0.08, 1), 0);
  assert.ok(saccadeCurve(0.5, 0.08, 1) > 0.8, "fast ease-out: most of the way by half time");
  assert.ok(Math.abs(saccadeCurve(1, 0.08, 1) - 1.08) < 1e-9);
  assert.equal(saccadeCurve(2.5, 0.08, 1), 1);
  assert.equal(saccadeCurve(1, 0, 0), 1);
});

test("small pointer jitter is ignored, the eyes hold", () => {
  const rig = new GazeRig("cozmo", 0, { x: 0, y: 0 }, () => 0.5);
  // Below the small threshold, wobbling.
  const track = run(rig, 0, 0.3, (t) => ({ want: { x: 0.03 * Math.sin(t * 40), y: 0 }, cursor: null, state: "idle", reduced: false }));
  for (const p of track) assert.ok(Math.abs(p.x) < 1e-9, `held at ${p.t}`);
});

test("a big move is a jump: 60-120 ms, a small overshoot, then a hold", () => {
  const rig = new GazeRig("cozmo", 0, { x: 0, y: 0 }, () => 0.5);
  const track = run(rig, 0, 0.6, { want: { x: 0.8, y: 0 }, cursor: null, state: "idle", reduced: false });
  const reach = track.find((p) => p.x >= 0.8);
  assert.ok(reach && reach.t >= SACCADE.durMin * 0.8 && reach.t <= SACCADE.durMax + 0.02, `reached at ${reach?.t}`);
  const peak = Math.max(...track.map((p) => p.x));
  assert.ok(peak > 0.82 && peak < 0.8 * (1 + SACCADE.overMax) + 1e-6, `overshoot ${peak}`);
  const late = track.filter((p) => p.t > SACCADE.durMax + SACCADE.settle + 0.02 && p.t < MICRO.quietFor);
  for (const p of late) assert.ok(Math.abs(p.x - 0.8) < 1e-9, "holds on the target");
});

test("a moving target is followed in discrete jumps, not continuously", () => {
  const rig = new GazeRig("cozmo", 0, { x: -0.8, y: 0 }, () => 0.5);
  // About 360 px/s at the island's reach.
  const track = run(rig, 0, 2.5, (t) => ({ want: { x: -0.8 + 0.6 * t, y: 0 }, cursor: null, state: "idle", reduced: false }));
  // Count holds: frames where the eye is (nearly) still.
  let held = 0;
  for (let i = 1; i < track.length; i++) if (Math.abs(track[i].x - track[i - 1].x) < 0.002) held++;
  assert.ok(held > track.length * 0.4, `held ${held} of ${track.length} frames`);
});

test("reduced motion: the eyes go straight to the target", () => {
  const rig = new GazeRig("head", 0, { x: 0, y: 0 });
  const out = rig.step(0.01, 0.01, { want: { x: 0.6, y: -0.2 }, cursor: null, state: "idle", reduced: true });
  assert.deepEqual(out.eye, { x: 0.6, y: -0.2 });
  assert.equal(rig.busy(0.01), false);
});

test("head style: the eyes lead, the head follows later and springs", () => {
  const rig = new GazeRig("head", 0, { x: 0, y: 0 }, () => 0.5);
  const track = [];
  let t = 0;
  for (let i = 0; i < 180; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, { want: { x: 0.9, y: 0 }, cursor: null, state: "listening", reduced: false });
    track.push({ t, head: rig.frame(t).head.x });
  }
  const at = (s) => track.find((p) => p.t >= s).head;
  assert.ok(at(0.12) < 0.3, `head still behind when the eyes have landed (${at(0.12)})`);
  const peak = Math.max(...track.map((p) => p.head));
  assert.ok(peak > 0.9 * 0.72, "the head overshoots its rest a little");
});

test("head style: a resting pointer gets boring, the eyes glance away", () => {
  const rig = new GazeRig("head", 0, { x: 0.8, y: 0 }, Math.random);
  rig.noteCursor({ x: 0.9, y: 0 }, 0);
  const track = run(rig, 0, 9, { want: { x: 0.8, y: 0 }, cursor: { x: 0.9, y: 0 }, state: "idle", reduced: false });
  const early = track.filter((p) => p.t < 3);
  const late = track.filter((p) => p.t > 4);
  assert.ok(early.every((p) => p.x > 0.55), "tracks while interested");
  assert.ok(late.some((p) => p.x < 0.4), "glances away once bored");
});

test("head style: a pointer close to the face brings the happy squint", () => {
  const rig = new GazeRig("head", 0, { x: 0, y: 0 }, () => 0.5);
  const near = { x: 0.05, y: 0.1 };
  rig.noteCursor(near, 0);
  run(rig, 0, 1.2, { want: near, cursor: near, state: "idle", reduced: false });
  const f = rig.frame(1.2);
  assert.equal(f.mode, "near");
  assert.ok(f.squint > 0.95);
  assert.ok(rig.fixation.y > 0.4, "looks down at it");
});

test("head style: a flick of the pointer startles", () => {
  const rig = new GazeRig("head", 0, { x: 0, y: 0 }, () => 0.5);
  let t = 0;
  for (let i = 0; i <= 6; i++) {
    rig.noteCursor({ x: -0.9 + i * 0.3, y: 0.2 }, t);
    t += 0.016;
  }
  assert.ok(rig.frame(t + 0.1).surprise > 0.5);
  assert.equal(rig.frame(t + 2).surprise, 0);
});

test("cozmo perspective: the eye on the gaze side grows, the far one shrinks and narrows, the gap closes", () => {
  const rest = placePill("cozmo", { x: 0, y: 0 }, { x: 0, y: 0 }, geom);
  assert.deepEqual(rest.eyes.map((e) => [e.sx, e.sy]), [[1, 1], [1, 1]]);
  const right = placePill("cozmo", { x: 1, y: 0 }, { x: 1, y: 0 }, geom);
  const [far, near] = right.eyes;
  assert.ok(near.sy > 1.15 && far.sy < 0.9 && far.sx < 0.8);
  assert.ok(near.x - far.x < rest.eyes[1].x - rest.eyes[0].x, "the gap closes");
  assert.ok(far.x > rest.eyes[0].x && near.x > rest.eyes[1].x, "both move toward the gaze side");
  assert.ok(right.hl.x < 0, "the highlight slides against the turn");
});

test("sphere: a turned point moves toward the turn and foreshortens near the limb", () => {
  const q = onSphere(30, 0, 80, (15 * Math.PI) / 180, 0, 3.2);
  assert.ok(q.x > 30 && q.sx < 1);
  const p = onSphere(-30, 0, 80, (15 * Math.PI) / 180, 0, 3.2);
  assert.ok(p.x > -30 && p.sx > q.sx);
});

test("classic stays the default: no style means the shipped sliding eyes", () => {
  assert.equal(styleOf({ face: presetFace("buddy") }), "classic");
  assert.equal(styleOf({ face: presetFace("buddy"), motionStyle: "head" }), "head");
  assert.equal(styleOf({ face: presetFace("mia"), motionStyle: "head" }), "classic", "the rig is for pill eyes only");
});
