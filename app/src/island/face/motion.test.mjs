// Motion tests. Run from app/: node --test src/island/face/*.test.mjs

import assert from "node:assert/strict";
import { test } from "node:test";
import { CHANNELS, liftCurve, nodCurve, reboundCurve, targetPose } from "./motion.ts";
import { FACE_STATES, presetFace } from "./schema.ts";

const near = (a, b, eps = 0.02) => Math.abs(a - b) <= eps;

test("accepted nod: down by 120 ms, back by 350 ms, settled by 800 ms", () => {
  assert.equal(nodCurve(0), 0);
  assert.ok(near(nodCurve(0.12), 1));
  assert.ok(nodCurve(0.35) <= 0, "back up (with a small rebound) by 350 ms");
  assert.equal(nodCurve(0.8), 0);
  assert.equal(nodCurve(2), 0);
});

test("approval lift: up in 180 ms, back by 430 ms, then holds", () => {
  assert.ok(near(liftCurve(0.18), 1));
  assert.ok(near(liftCurve(0.43), 0));
  assert.equal(liftCurve(3), 0);
});

test("reduced motion: every state has one fixed pose, whatever the clock says", () => {
  for (const face of [presetFace("mia"), presetFace("buddy")]) {
    for (const state of FACE_STATES) {
      const at = (st, t) =>
        targetPose({ face, state, st, t, look: { x: 0.2, y: 0.3 }, lean: { x: 0.2, y: 0.3 }, level: 0, emote: null, reduced: true, compact: true });
      const a = at(0.05, 1);
      const b = at(2.7, 99);
      for (const ch of CHANNELS) assert.equal(a[ch], b[ch], `${face.preset}.${state}.${ch}`);
    }
  }
});

test("thinking stays mostly open, working is two-thirds open and level", () => {
  const face = presetFace("cat");
  const pose = (state) =>
    targetPose({ face, state, st: 1, t: 1, look: { x: 0, y: 0 }, lean: { x: 0, y: 0 }, level: 0, emote: null, reduced: false, compact: true });
  const th = pose("thinking");
  const wk = pose("working");
  const ap = (p, s) => 1 - p[`u${s}`] - p[`l${s}`];
  assert.ok(ap(th, "L") >= 0.85 && ap(th, "R") >= 0.8, "thinking open");
  assert.ok(ap(wk, "L") >= 0.55 && ap(wk, "L") <= 0.72, "working two-thirds");
  assert.equal(wk.uL, wk.uR);
  assert.ok(th.eyL < 0, "one eye lifted");
});

test("offline turns the eyes into capsules; listening looks straight ahead", () => {
  const face = presetFace("classic");
  const base = { face, st: 1, t: 1, look: { x: 0.9, y: 0.9 }, lean: { x: 0.9, y: 0.9 }, level: 0.5, emote: null, reduced: false, compact: true };
  assert.equal(targetPose({ ...base, state: "offline" }).off, 1);
  const l = targetPose({ ...base, state: "listening" });
  assert.equal(l.gx, 0);
  assert.ok(Math.abs(l.gy) < 0.15);
});

test("pill eyes: sleeping is a closed bar, done a happy arch, listening taller, error smaller", () => {
  const face = presetFace("buddy");
  const pose = (state, level = 0) =>
    targetPose({ face, state, st: 1, t: 1, look: { x: 0, y: 0 }, lean: { x: 0, y: 0 }, level, emote: null, reduced: false, compact: true });
  const sl = pose("sleeping");
  assert.ok(sl.uL + sl.lL >= 0.9 && sl.uR + sl.lR >= 0.9, "sleeping closed");
  const dn = pose("done");
  assert.ok(dn.lL > 0.6 && dn.lowCurve > 0.9, "done arch");
  assert.equal(dn.open, 0, "done keeps the thin smile");
  assert.ok(pose("listening", 1).stretch > pose("listening", 0).stretch && pose("listening", 0).stretch > 0);
  const er = pose("error");
  assert.ok(er.sL < 1 && er.sR < 1 && er.tL < 0, "error: smaller, worried");
  // Pill eyes have no lids to follow the gaze; other shapes still do.
  const down = (f) => targetPose({ face: f, state: "idle", st: 1, t: 1, look: { x: 0, y: 1 }, lean: { x: 0, y: 1 }, level: 0, emote: null, reduced: false, compact: true });
  assert.equal(down(face).uL, 0);
  assert.ok(down(presetFace("classic")).uL > 0);
});

test("pill blink rebound: a small wave that dies out", () => {
  assert.equal(reboundCurve(-0.1), 0);
  assert.equal(reboundCurve(0), 0);
  assert.ok(reboundCurve(0.065) > 0.3, "lands low first");
  assert.ok(reboundCurve(0.2) < 0, "then rises past rest");
  assert.equal(reboundCurve(1), 0);
  for (let e = 0; e < 0.5; e += 0.01) assert.ok(Math.abs(reboundCurve(e)) <= 1);
});
