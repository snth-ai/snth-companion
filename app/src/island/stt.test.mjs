// The island's speech-to-text bits. Run from app/: node --test src/island/*.test.mjs

import assert from "node:assert/strict";
import { test } from "node:test";
import { aboutMb, sttEngines, sttLoadingVisible, tailOf, transcriptVisible } from "./sttView.ts";

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

test("a quiet loading line while the model loads during a hold, until words come", () => {
  const base = { hold: "voice", stt_loading: true, transcript: "" };
  assert.equal(sttLoadingVisible(base), true);
  assert.equal(sttLoadingVisible({ ...base, transcript: null }), true);
  assert.equal(sttLoadingVisible({ ...base, transcript: "Найди" }), false, "the words win");
  assert.equal(sttLoadingVisible({ ...base, stt_loading: false }), false);
  assert.equal(sttLoadingVisible({ ...base, hold: null }), false, "not after the release");
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

test("Windows runs Parakeet alone, whatever the settings say, and offers no choice", () => {
  const mac = { engine: "whisper", choices: ["whisper", "parakeet"] };
  assert.deepEqual(sttEngines(mac, "whisper"), { chosen: "whisper", engines: ["whisper", "parakeet"], pick: true });
  assert.deepEqual(sttEngines({ ...mac, engine: "parakeet" }, "parakeet").chosen, "parakeet");
  // The stored choice is whisper (the default); Parakeet runs.
  const win = { engine: "parakeet", choices: ["parakeet"] };
  assert.deepEqual(sttEngines(win, "whisper"), { chosen: "parakeet", engines: ["parakeet"], pick: false });
  // Even a backend that reports the stored choice: the build's engine.
  assert.equal(sttEngines({ choices: ["parakeet"] }, "whisper").chosen, "parakeet");
  // An older backend (no fields): a Mac with both.
  assert.deepEqual(sttEngines({}, undefined), { chosen: "whisper", engines: ["whisper", "parakeet"], pick: true });
  // Its download, about 640 MiB (the encoder, decoder, tokens, ONNX detector).
  assert.equal(aboutMb(652_183_999 + 18_202_004 + 93_939 + 2_327_524), 640);
});
