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

test("pill eyes default to smooth (C2); every other shape stays classic", () => {
  assert.equal(styleOf({ face: presetFace("buddy") }), "smooth");
  assert.equal(styleOf({ face: presetFace("buddy"), motionStyle: "classic" }), "classic", "the one-line rollback");
  assert.equal(styleOf({ face: presetFace("buddy"), motionStyle: "head" }), "head");
  assert.equal(styleOf({ face: presetFace("mia") }), "classic");
  assert.equal(styleOf({ face: presetFace("mia"), motionStyle: "head" }), "classic", "the rig is for pill eyes only");
});

// ---------- smooth (C2) ----------

import { SMOOTH, SMOOTH_DEFAULTS, SmoothRig, makeRig, smoothDerived } from "./rig.ts";

const idle = (want, cursor = null) => ({ want, cursor, state: "idle", reduced: false });

test("smooth: a slowly moving target is followed by a glide, never held still and never jumped", () => {
  const rig = new SmoothRig(0, { x: -0.6, y: 0 }, () => 0.9);
  let t = 0;
  let prev = -0.6;
  let held = 0;
  let maxStep = 0;
  for (let i = 0; i < 240; i++) {
    t += 1 / 120;
    const out = rig.step(t, 1 / 120, idle({ x: -0.6 + 0.5 * t, y: 0 }));
    const step = Math.abs(out.eye.x - prev);
    if (i > 10 && step < 1e-4) held++;
    maxStep = Math.max(maxStep, step);
    prev = out.eye.x;
    assert.equal(rig.frame(t).saccading, false, `no saccade at ${t.toFixed(2)}`);
  }
  assert.equal(held, 0, "no stair-step holds");
  assert.ok(maxStep < 0.02, `continuous (largest step per frame ${maxStep})`);
});

test("smooth: a big jump is an eased saccade of 140-220 ms with a 2-4% overshoot", () => {
  const d = smoothDerived(SMOOTH_DEFAULTS);
  assert.ok(Math.abs(d.durShort - 0.14) < 1e-9 && Math.abs(d.durLong - 0.22) < 1e-9);
  assert.ok(SMOOTH.overshoot >= 0.02 && SMOOTH.overshoot <= 0.04);
  const rig = new SmoothRig(0, { x: -0.5, y: 0 }, () => 0.9);
  let t = 0;
  const xs = [];
  for (let i = 0; i < 120; i++) {
    t += 1 / 240;
    xs.push({ t, x: rig.step(t, 1 / 240, idle({ x: 0.5, y: 0 })).eye.x });
  }
  const peak = Math.max(...xs.map((p) => p.x));
  assert.ok(peak > 0.5 && peak <= 0.5 + 1.0 * SMOOTH.overshoot + 1e-6, `overshoot ${peak}`);
  const reached = xs.find((p) => p.x >= 0.5);
  assert.ok(reached.t > 0.1 && reached.t < 0.24, `reached at ${reached.t}`);
  // Eased: the first and last steps of the jump are small, no velocity snap.
  const steps = xs.slice(1).map((p, i) => Math.abs(p.x - xs[i].x));
  const biggest = Math.max(...steps);
  assert.ok(steps[0] < biggest * 0.25, "eases in");
  const end = xs.at(-1).x;
  assert.ok(Math.abs(end - 0.5) < 1e-6, "settles on the target");
});

test("smooth: the head follows without wobble at the default damping", () => {
  const rig = new SmoothRig(0, { x: 0, y: 0 }, () => 0.9);
  let t = 0;
  let peak = 0;
  for (let i = 0; i < 480; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, { want: { x: 0.9, y: 0 }, cursor: null, state: "listening", reduced: false });
    peak = Math.max(peak, rig.frame(t).head.x);
  }
  const rest = 0.9 * SMOOTH.headFollow;
  assert.ok(peak < rest * 1.02, `overshoot at most 2% (peak ${peak}, rest ${rest})`);
  assert.ok(Math.abs(rig.frame(t).head.x - rest) < 0.01);
});

test("smooth: a pointer passing the face does not squint; a dwelling one does, easing in", () => {
  const rig = new SmoothRig(0, { x: 0, y: 0 }, () => 0.9);
  const near = { x: 0.05, y: 0.1 };
  let t = 0;
  for (let i = 0; i < 36; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, idle(near, near));
  }
  assert.equal(rig.frame(t).mode, "track", "0.3 s near is only passing by");
  for (let i = 0; i < 36; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, idle({ x: 0.8, y: 0 }, { x: 0.8, y: 0 }));
  }
  for (let i = 0; i < 60; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, idle(near, near));
  }
  assert.equal(rig.frame(t).mode, "near");
  const early = rig.frame(t).squint;
  for (let i = 0; i < 120; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, idle(near, near));
  }
  assert.ok(early < 0.5 && rig.frame(t).squint > 0.9, `eases in (${early} then ${rig.frame(t).squint})`);
});

test("smooth: the startle eases in and out", () => {
  const rig = new SmoothRig(0, { x: 0, y: 0 }, () => 0.9);
  let t = 0;
  for (let i = 0; i <= 6; i++) {
    rig.noteCursor({ x: -0.9 + i * 0.3, y: 0.2 }, t);
    t += 0.016;
  }
  // Onset: the first moment with any startle.
  let start = 0;
  while (rig.surpriseLevel(start + 0.0005) === 0 && start < 1) start += 0.0005;
  assert.ok(rig.surpriseLevel(start + 0.03) < 0.15, "no pop at the start");
  assert.ok(rig.surpriseLevel(start + 0.25) > 0.6, "full at the hold");
  assert.ok(rig.surpriseLevel(start + 0.7) < 0.4, "eases out");
  assert.equal(rig.surpriseLevel(start + 1), 0);
});

test("smooth: live tuning applies, breath 0 lets the loop rest, makeRig picks the class", () => {
  const rig = makeRig("smooth", 0, { x: 0, y: 0 }, { turn: 24, breath: 0 });
  assert.ok(rig instanceof SmoothRig);
  rig.step(0.01, 0.01, idle({ x: 0, y: 0 }));
  assert.equal(rig.frame(0.01).yaw, 24);
  assert.equal(rig.busy(0.01), false, "settled and no breath: nothing to draw");
  assert.equal(rig.ambientFps(false), 0);
  rig.setTuning({ breath: 1 });
  assert.equal(rig.busy(0.02), false, "settled breathing is not busy");
  assert.equal(rig.ambientFps(false), SMOOTH.breathFps, "it runs at the capped rate");
  assert.equal(rig.ambientFps(true), 0, "no breathing with reduced motion");
  assert.equal(makeRig("classic", 0, { x: 0, y: 0 }), null);
});

// ---------- smooth in the island ----------

test("island: 30 Hz pointer samples give a glide without ripple", () => {
  const rig = new SmoothRig(0, { x: -0.5, y: 0 }, () => 0.9);
  let t = 0;
  let want = { x: -0.5, y: 0 };
  const xs = [];
  for (let i = 0; i < 240; i++) {
    t += 1 / 120;
    // A new sample every 33 ms, the shell's moving poll.
    if (i % 4 === 0) want = { x: -0.5 + 0.4 * t, y: 0 };
    xs.push(rig.step(t, 1 / 120, idle(want)).eye.x);
  }
  const v = xs.slice(1).map((x, i) => (x - xs[i]) * 120);
  const steady = v.slice(80);
  const mean = steady.reduce((a, b) => a + b, 0) / steady.length;
  const ripple = Math.max(...steady.map((x) => Math.abs(x - mean)));
  assert.ok(ripple < mean * 0.2, `velocity ripple ${ripple.toFixed(3)} vs speed ${mean.toFixed(3)}`);
});

test("island: the first sample after a pause is not a flick", () => {
  const rig = new SmoothRig(0, { x: 0, y: 0 }, () => 0.9);
  rig.noteCursor({ x: -0.8, y: 0.3 }, 0);
  // The pointer rested (no samples), then one sample far away.
  rig.noteCursor({ x: 0.8, y: 0.3 }, 1.5);
  assert.equal(rig.surpriseLevel(1.7), 0);
  // A real flick at 30 Hz does startle.
  let t = 3;
  for (let i = 0; i < 5; i++) {
    rig.noteCursor({ x: -0.8 + i * 0.35, y: 0.3 }, t);
    t += 1 / 30;
  }
  assert.ok(rig.surpriseLevel(t + 0.2) > 0.5);
});

test("island: the synth's own pointer is followed, never startles or squints", () => {
  const rig = new SmoothRig(0, { x: 0, y: 0 }, () => 0.9);
  const agent = { agent: true };
  let t = 0;
  for (let i = 0; i < 6; i++) {
    rig.noteCursor({ x: -0.9 + i * 0.35, y: 0.1 }, t, true);
    t += 1 / 30;
  }
  assert.equal(rig.surpriseLevel(t + 0.2), 0, "no startle");
  const near = { x: 0.03, y: 0.05 };
  for (let i = 0; i < 240; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, { want: near, cursor: near, state: "idle", reduced: false, attention: { ...agent, near: true } });
  }
  assert.notEqual(rig.frame(t).mode, "near", "no squint");
  assert.equal(rig.frame(t).saccading, false);
});

test("island: the island's own shape decides near, not the distance", () => {
  const rig = new SmoothRig(0, { x: 0, y: 0 }, () => 0.9);
  const close = { x: 0.05, y: 0.02 };
  let t = 0;
  // Close by distance but not on the shape (e.g. the menu bar beside the ear).
  for (let i = 0; i < 120; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, { want: close, cursor: close, state: "idle", reduced: false, attention: { near: false } });
  }
  assert.equal(rig.frame(t).mode, "track");
  // Far by distance but on the open island's shape: squints after the dwell.
  const onShape = { x: 0.4, y: 0.1 };
  for (let i = 0; i < 120; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, { want: onShape, cursor: onShape, state: "idle", reduced: false, attention: { near: true } });
  }
  assert.equal(rig.frame(t).mode, "near");
});

test("island: no boredom while the owner is busy with the island", () => {
  const rig = new SmoothRig(0, { x: 0.6, y: 0.2 }, Math.random);
  rig.noteCursor({ x: 0.6, y: 0.2 }, 0);
  let t = 0;
  for (let i = 0; i < 120 * 10; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, { want: { x: 0.6, y: 0.2 }, cursor: { x: 0.6, y: 0.2 }, state: "idle", reduced: false, attention: { engaged: true } });
    assert.notEqual(rig.frame(t).mode, "bored", `bored at ${t.toFixed(2)}`);
  }
  // Released: boredom comes back.
  for (let i = 0; i < 120 * 2; i++) {
    t += 1 / 120;
    rig.step(t, 1 / 120, { want: { x: 0.6, y: 0.2 }, cursor: { x: 0.6, y: 0.2 }, state: "idle", reduced: false, attention: { engaged: false } });
  }
  assert.equal(rig.frame(t).mode, "bored");
});

test("island: reduced motion goes straight to the target, no breath, no boredom", () => {
  const rig = new SmoothRig(0, { x: 0, y: 0 }, () => 0.9);
  let t = 0;
  for (let i = 0; i < 120 * 6; i++) {
    t += 1 / 120;
    const out = rig.step(t, 1 / 120, { want: { x: 0.7, y: 0.1 }, cursor: { x: 0.7, y: 0.1 }, state: "idle", reduced: true });
    assert.deepEqual(out.eye, { x: 0.7, y: 0.1 });
  }
  assert.equal(rig.frame(t).breathe, 0);
  assert.equal(rig.frame(t).mode, "track");
  assert.equal(rig.ambientFps(true), 0);
});
