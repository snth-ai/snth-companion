// The island's speech-to-text bits. Run from app/: node --test src/island/*.test.mjs

import assert from "node:assert/strict";
import { test } from "node:test";
import { aboutMb, tailOf, transcriptVisible } from "./sttView.ts";

test("the live transcript keeps its end, from a word", () => {
  assert.equal(tailOf("  Найди мне рейс  "), "Найди мне рейс");
  const long = "раз два три четыре пять шесть семь восемь девять десять ".repeat(4).trim();
  const t = tailOf(long, 40);
  assert.ok(t.startsWith("…"), t);
  assert.ok(long.endsWith(t.slice(1)), t);
  assert.ok(t.length <= 41);
  assert.ok(!t.slice(1).startsWith(" "));
  // One very long word: cut anyway.
  assert.equal(tailOf("a".repeat(200), 50), "…" + "a".repeat(50));
});

test("the offer says about 550 MB for the pinned files", () => {
  assert.equal(aboutMb(574_041_195 + 885_098), 550);
});

test("the words show while held, while sent, and briefly after", () => {
  const base = { transcript: "Найди рейс", hold: null, sending: false, message: null };
  assert.equal(transcriptVisible({ ...base, hold: "voice" }, 0), true);
  assert.equal(transcriptVisible({ ...base, sending: true }, 0), true);
  assert.equal(transcriptVisible({ ...base, message: { stage: "accepted", at_ms: 1000 } }, 2000), true);
  assert.equal(transcriptVisible({ ...base, message: { stage: "accepted", at_ms: 1000 } }, 3000), false);
  assert.equal(transcriptVisible({ ...base, message: { stage: "working", at_ms: 1000 } }, 1100), false);
  assert.equal(transcriptVisible(base, 0), false, "idle: nothing");
  assert.equal(transcriptVisible({ ...base, transcript: "  ", hold: "voice" }, 0), false);
  assert.equal(transcriptVisible({ ...base, transcript: null, hold: "voice" }, 0), false);
});
