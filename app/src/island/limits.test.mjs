// The island's subscription limits. Run from app/: node --test src/island/*.test.mjs

import assert from "node:assert/strict";
import { test } from "node:test";
import { levelOf, limitsModel, planTitle, resetText, windowLabel } from "./limitsView.ts";

// 2026-10-28 08:00 UTC = 16:00 in Bali (WITA, UTC+8), a Wednesday.
const NOW = Date.parse("2026-10-28T08:00:00Z");
const BALI = "Asia/Makassar";

const claude = (over = {}) => ({
  provider: "claude",
  plan: "Claude Max",
  windows: [
    { id: "five_hour", label_ru: "5ч", label_en: "5h", used_pct: 34, resets_at: "2026-10-28T10:20:00Z" },
    { id: "seven_day", label_ru: "неделя", label_en: "week", used_pct: 61.4, resets_at: "2026-11-01T19:00:00Z" },
  ],
  blocked: false,
  fetched_at: "2026-10-28T07:59:00Z",
  received_ms: NOW - 60_000,
  ...over,
});

test("the reset time: today the time only, another day the weekday, later the date", () => {
  assert.equal(resetText("2026-10-28T10:20:00Z", NOW, "ru", BALI), "18:20");
  assert.equal(resetText("2026-10-28T10:20:00Z", NOW, "en", BALI), "18:20");
  // Sunday 19:00 UTC = Monday 03:00 in Bali.
  assert.equal(resetText("2026-11-01T19:00:00Z", NOW, "ru", BALI), "пн 03:00");
  assert.equal(resetText("2026-11-01T19:00:00Z", NOW, "en", BALI), "Mon 03:00");
  // The same instant in UTC is still Sunday.
  assert.equal(resetText("2026-11-01T19:00:00Z", NOW, "en", "UTC"), "Sun 19:00");
  // Tomorrow is a weekday too (not "tomorrow").
  assert.equal(resetText("2026-10-29T01:00:00Z", NOW, "en", BALI), "Thu 09:00");
  // Past midnight here is another day even within hours.
  assert.equal(resetText("2026-10-28T16:30:00Z", NOW, "ru", BALI), "чт 00:30");
  // A week or more away: the date.
  assert.equal(resetText("2026-11-05T02:00:00Z", NOW, "en", BALI), "5 Nov 10:00");
  assert.match(resetText("2026-11-05T02:00:00Z", NOW, "ru", BALI), /^5 ноя\S* 10:00$/);
  // Nothing usable.
  assert.equal(resetText("", NOW, "en", BALI), "");
  assert.equal(resetText("soon", NOW, "en", BALI), "");
});

test("thresholds: below 80 nothing, 80 yellow, 95 red", () => {
  assert.equal(levelOf(0), "ok");
  assert.equal(levelOf(79), "ok");
  assert.equal(levelOf(80), "warn");
  assert.equal(levelOf(94), "warn");
  assert.equal(levelOf(95), "high");
  assert.equal(levelOf(100), "high");
});

test("Claude 34/61: two rows, no dot", () => {
  const m = limitsModel(claude(), "ru", NOW, BALI);
  assert.equal(m.title, "Claude Max");
  assert.deepEqual(
    m.rows.map((r) => [r.label, r.pct, r.level, r.reset]),
    [
      ["5ч", 34, "ok", "18:20"],
      ["неделя", 61, "ok", "пн 03:00"],
    ],
  );
  assert.equal(m.dot, null);
  assert.equal(m.blocked, false);
  assert.equal(m.ageMin, null);
  assert.deepEqual(
    limitsModel(claude(), "en", NOW, BALI).rows.map((r) => r.label),
    ["5h", "week"],
  );
});

test("Codex at 85%: yellow; at 95% or refused: red", () => {
  const codex = (pct, blocked = false) =>
    limitsModel(
      claude({
        provider: "codex",
        plan: "ChatGPT Plus",
        blocked,
        windows: [
          { id: "five_hour", label_ru: "5ч", label_en: "5h", used_pct: pct, resets_at: "2026-10-28T09:00:00Z" },
          { id: "seven_day", label_ru: "неделя", label_en: "week", used_pct: 40, resets_at: "2026-11-02T00:00:00Z" },
        ],
      }),
      "en",
      NOW,
      BALI,
    );
  assert.equal(codex(85).dot, "warn");
  assert.equal(codex(85).rows[0].level, "warn");
  assert.equal(codex(95).dot, "high");
  assert.equal(codex(79.6).dot, "warn", "rounded to 80");
  assert.equal(codex(79.4).dot, null);
  // Refused: red even below the thresholds, on the fullest window.
  const b = codex(70, true);
  assert.equal(b.blocked, true);
  assert.equal(b.dot, "high");
  assert.deepEqual(b.rows.map((r) => r.level), ["high", "ok"]);
});

test("a refusal with no windows still shows; nothing at all shows nothing", () => {
  const b = limitsModel(claude({ windows: [], blocked: true }), "en", NOW, BALI);
  assert.equal(b.dot, "high");
  assert.deepEqual(b.rows, []);
  assert.equal(limitsModel(claude({ windows: [] }), "en", NOW, BALI), null);
  assert.equal(limitsModel(null, "en", NOW, BALI), null);
  assert.equal(limitsModel(undefined, "en", NOW, BALI), null);
  // A malformed window is skipped.
  const m = limitsModel(claude({ windows: [{ id: "x", label_en: "x", used_pct: "50" }, { id: "y", label_en: "y", used_pct: 50, resets_at: "" }] }), "en", NOW);
  assert.deepEqual(m.rows.map((r) => [r.id, r.reset]), [["y", ""]]);
});

test("a window past its reset time shows 0% reset, and ends a refusal", () => {
  const later = Date.parse("2026-10-28T10:21:00Z");
  const m = limitsModel(claude({ blocked: true, windows: [{ id: "five_hour", label_ru: "5ч", label_en: "5h", used_pct: 100, resets_at: "2026-10-28T10:20:00Z" }, { id: "seven_day", label_ru: "", label_en: "", used_pct: 61, resets_at: "2026-11-01T19:00:00Z" }] }), "en", later, BALI);
  assert.deepEqual(m.rows[0], { id: "five_hour", label: "5h", pct: 0, level: "ok", reset: "", wasReset: true });
  assert.equal(m.blocked, false);
  assert.equal(m.dot, null);
  // The default label when the synth sent none.
  assert.equal(m.rows[1].label, "week");
});

test("an old snapshot says how old", () => {
  assert.equal(limitsModel(claude({ fetched_at: "2026-10-28T07:31:00Z" }), "en", NOW, BALI).ageMin, null);
  assert.equal(limitsModel(claude({ fetched_at: "2026-10-28T07:20:00Z" }), "en", NOW, BALI).ageMin, 40);
  // No fetched_at: the time this computer received it.
  assert.equal(limitsModel(claude({ fetched_at: "", received_ms: NOW - 3 * 3_600_000 }), "en", NOW, BALI).ageMin, 180);
  // A synth clock ahead of ours is not "old".
  assert.equal(limitsModel(claude({ fetched_at: "2026-10-28T09:00:00Z" }), "en", NOW, BALI).ageMin, null);
});

test("titles and labels fall back sensibly", () => {
  assert.equal(planTitle({ plan: "", provider: "claude" }), "Claude");
  assert.equal(planTitle({ plan: " ", provider: "codex" }), "ChatGPT");
  assert.equal(planTitle({ plan: "ChatGPT Pro", provider: "codex" }), "ChatGPT Pro");
  assert.equal(windowLabel({ id: "seven_day_opus", label_ru: "неделя Opus", label_en: "" }, "en"), "неделя Opus");
  assert.equal(windowLabel({ id: "five_hour", label_ru: "", label_en: "" }, "ru"), "5ч");
  assert.equal(windowLabel({ id: "odd", label_ru: "", label_en: "" }, "ru"), "odd");
});
