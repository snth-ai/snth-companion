// The island's subscription limits. Run from app/: node --test src/island/*.test.mjs

import assert from "node:assert/strict";
import { test } from "node:test";
import {
  closedFallbacks,
  closedParts,
  closedRows,
  compactLines,
  DEFAULT_DISPLAY,
  displayOf,
  fullest,
  levelOf,
  limitsModel,
  openReset,
  planTitle,
  resetText,
  shownPct,
  untilShort,
  untilText,
  windowLabel,
  windowLetter,
} from "./limitsView.ts";

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
  assert.deepEqual(m.rows[0], { id: "five_hour", label: "5h", letter: "h", pct: 0, level: "ok", reset: "", resetAt: null, wasReset: true });
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

// --- how they show (tz-island-limits-display.md) ---

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const show = (over = {}) => ({ ...DEFAULT_DISPLAY, ...over });
const texts = (m, d, now = NOW) => closedParts(m, d, now, "limit").map((p) => [p.text, p.level]);

test("the countdown: minutes under an hour, hours under a day, days after, all rounded up", () => {
  assert.equal(untilShort(45 * MIN), "45m");
  assert.equal(untilShort(30_000), "1m");
  assert.equal(untilShort(59 * MIN + 1), "1h", "59.x minutes round up to the hour");
  assert.equal(untilShort(HOUR), "1h");
  assert.equal(untilShort(HOUR + 1), "2h");
  assert.equal(untilShort(2.3 * HOUR), "3h");
  assert.equal(untilShort(23.5 * HOUR), "24h");
  assert.equal(untilShort(DAY), "1d");
  assert.equal(untilShort(2.1 * DAY), "3d");
  assert.equal(untilShort(0), "");
  assert.equal(untilShort(-5 * MIN), "");
  assert.equal(untilShort(Number.NaN), "");
  // The open island's, in its language.
  assert.equal(untilText(2 * HOUR, "ru"), "2ч");
  assert.equal(untilText(45 * MIN, "ru"), "45м");
  assert.equal(untilText(3 * DAY, "ru"), "3д");
  assert.equal(untilText(3 * DAY, "en"), "3d");
});

test("the window's letter", () => {
  assert.equal(windowLetter({ id: "five_hour", label_ru: "5ч", label_en: "5h" }), "h");
  assert.equal(windowLetter({ id: "seven_day", label_ru: "неделя", label_en: "week" }), "w");
  assert.equal(windowLetter({ id: "seven_day_opus", label_ru: "неделя Opus", label_en: "week Opus" }), "wO");
  assert.equal(windowLetter({ id: "seven_day_sonnet", label_ru: "", label_en: "" }), "wS", "no label: from the id");
  assert.equal(windowLetter({ id: "seven_day_opus", label_ru: "неделя", label_en: "" }), "wO");
  assert.equal(windowLetter({ id: "monthly", label_ru: "", label_en: "month" }), "mo");
});

test("the display settings: missing or unknown fields are the defaults", () => {
  assert.deepEqual(displayOf(undefined), DEFAULT_DISPLAY);
  assert.deepEqual(displayOf(null), DEFAULT_DISPLAY);
  assert.deepEqual(displayOf({ closed: "dot", window: "month", open: "full" }), show({ closed: "dot", open: "full" }));
  assert.deepEqual(DEFAULT_DISPLAY, { closed: "text", window: "auto", percent: "used", closed_reset: "in", open: "compact", open_reset: "in" });
});

test("closed, Claude 34/61: the fullest window by default, each window, both", () => {
  const m = limitsModel(claude(), "ru", NOW, BALI);
  // 5h resets at 10:20 UTC: 2 h 20 min from 08:00, rounded up to 3h; the
  // week at Sun 19:00 UTC: 4 d 11 h, rounded up to 5d.
  assert.deepEqual(texts(m, show()), [["w:61% 5d", "ok"]]);
  assert.deepEqual(texts(m, show({ window: "five_hour" })), [["h:34% 3h", "ok"]]);
  assert.deepEqual(texts(m, show({ window: "week" })), [["w:61% 5d", "ok"]]);
  assert.deepEqual(texts(m, show({ window: "both" })), [
    ["h:34% 3h", "ok"],
    ["w:61% 5d", "ok"],
  ]);
  // Left instead of used; no countdown.
  assert.deepEqual(texts(m, show({ window: "both", percent: "left", closed_reset: "off" })), [
    ["h:66%", "ok"],
    ["w:39%", "ok"],
  ]);
  // The text is the same in both languages.
  assert.deepEqual(texts(limitsModel(claude(), "en", NOW, BALI), show()), [["w:61% 5d", "ok"]]);
});

test("closed: a tie goes to the 5 hours; a missing window falls back to the fullest", () => {
  const tie = limitsModel(
    claude({
      windows: [
        { id: "seven_day", label_ru: "неделя", label_en: "week", used_pct: 50, resets_at: "2026-11-01T19:00:00Z" },
        { id: "five_hour", label_ru: "5ч", label_en: "5h", used_pct: 50, resets_at: "2026-10-28T08:45:00Z" },
      ],
    }),
    "en",
    NOW,
    BALI,
  );
  assert.equal(fullest(tie).id, "five_hour");
  assert.deepEqual(texts(tie, show()), [["h:50% 45m", "ok"]]);
  const weekOnly = limitsModel(claude({ windows: [{ id: "seven_day", label_ru: "", label_en: "", used_pct: 20, resets_at: "" }] }), "en", NOW, BALI);
  assert.deepEqual(closedRows(weekOnly, "five_hour").map((r) => r.id), ["seven_day"]);
  assert.deepEqual(texts(weekOnly, show({ window: "both" })), [["w:20%", "ok"]], "no resets_at: no countdown");
  // The Opus week counts in the fullest.
  const opus = limitsModel(
    claude({
      windows: [
        { id: "five_hour", label_ru: "5ч", label_en: "5h", used_pct: 12, resets_at: "2026-10-28T12:00:00Z" },
        { id: "seven_day", label_ru: "неделя", label_en: "week", used_pct: 58, resets_at: "2026-11-01T19:00:00Z" },
        { id: "seven_day_opus", label_ru: "неделя Opus", label_en: "week Opus", used_pct: 81, resets_at: "2026-11-01T19:00:00Z" },
      ],
    }),
    "en",
    NOW,
    BALI,
  );
  assert.deepEqual(texts(opus, show()), [["wO:81% 5d", "warn"]]);
});

test("closed: yellow from 80, red from 95 or refused; a reset window is 0% without a countdown", () => {
  const codex = (pct, blocked = false) =>
    limitsModel(
      claude({
        provider: "codex",
        plan: "ChatGPT Plus",
        blocked,
        windows: [
          { id: "five_hour", label_ru: "5ч", label_en: "5h", used_pct: pct, resets_at: "2026-10-28T09:36:00Z" },
          { id: "seven_day", label_ru: "неделя", label_en: "week", used_pct: 47, resets_at: "2026-11-01T19:00:00Z" },
        ],
      }),
      "en",
      NOW,
      BALI,
    );
  assert.deepEqual(texts(codex(85), show()), [["h:85% 2h", "warn"]]);
  assert.deepEqual(texts(codex(85), show({ percent: "left" })), [["h:15% 2h", "warn"]], "the colour follows the use");
  assert.deepEqual(texts(codex(96), show()), [["h:96% 2h", "high"]]);
  // Refused: every part red, whatever its percent.
  assert.deepEqual(texts(codex(70, true), show({ window: "both" })), [
    ["h:70% 2h", "high"],
    ["w:47% 5d", "high"],
  ]);
  // Refused with no window at all: the word, in red.
  const bare = limitsModel(claude({ windows: [], blocked: true }), "en", NOW, BALI);
  assert.deepEqual(texts(bare, show()), [["limit", "high"]]);
  // Past the reset time: 0%, no countdown, the refusal over.
  const later = Date.parse("2026-10-28T09:40:00Z");
  const m = limitsModel(claude({ blocked: true, windows: [{ id: "five_hour", label_ru: "", label_en: "", used_pct: 100, resets_at: "2026-10-28T09:36:00Z" }] }), "en", later, BALI);
  assert.deepEqual(texts(m, show(), later), [["h:0%", "ok"]]);
  assert.deepEqual(texts(m, show({ percent: "left" }), later), [["h:100%", "ok"]]);
});

test("closed: what the text steps down to when it does not fit", () => {
  assert.deepEqual(closedFallbacks(show({ window: "both" })), [
    show({ window: "both" }),
    show({ window: "auto" }),
    show({ window: "auto", closed_reset: "off" }),
  ]);
  assert.deepEqual(closedFallbacks(show({ window: "week", closed_reset: "off" })), [show({ window: "week", closed_reset: "off" })]);
  assert.deepEqual(closedFallbacks(show()), [show(), show({ closed_reset: "off" })]);
});

test("the percent shown: used or left", () => {
  assert.equal(shownPct(34, { percent: "used" }), 34);
  assert.equal(shownPct(34, { percent: "left" }), 66);
  assert.equal(shownPct(0, { percent: "left" }), 100);
});

// The island's words for the open view, as en.ts / ru.ts have them.
const WORDS = {
  en: {
    limitsWasReset: "reset",
    limitsIn: (x) => `in ${x}`,
    limitsResetsIn: (x) => `resets in ${x}`,
    limitsResets: (x) => `resets ${x}`,
    limitsBlocked: "limit reached",
    limitsAge: (min) => `updated ${min} min ago`,
  },
  ru: {
    limitsWasReset: "сброшено",
    limitsIn: (x) => `через ${x}`,
    limitsResetsIn: (x) => `сброс через ${x}`,
    limitsResets: (x) => `до ${x}`,
    limitsBlocked: "лимит исчерпан",
    limitsAge: (min) => `обновлено ${min} мин назад`,
  },
};

test("open: the reset as time left or by the clock, in the island's language", () => {
  for (const lang of ["en", "ru"]) {
    const m = limitsModel(claude(), lang, NOW, BALI);
    const [h, w] = m.rows;
    const at = (d, long) => [openReset(h, d, WORDS[lang], lang, NOW, long), openReset(w, d, WORDS[lang], lang, NOW, long)];
    if (lang === "en") {
      assert.deepEqual(at({ open_reset: "in" }, false), ["in 3h", "in 5d"]);
      assert.deepEqual(at({ open_reset: "in" }, true), ["resets in 3h", "resets in 5d"]);
      assert.deepEqual(at({ open_reset: "clock" }, false), ["18:20", "Mon 03:00"]);
      assert.deepEqual(at({ open_reset: "clock" }, true), ["resets 18:20", "resets Mon 03:00"]);
    } else {
      assert.deepEqual(at({ open_reset: "in" }, false), ["через 3ч", "через 5д"]);
      assert.deepEqual(at({ open_reset: "in" }, true), ["сброс через 3ч", "сброс через 5д"]);
      assert.deepEqual(at({ open_reset: "clock" }, false), ["18:20", "пн 03:00"]);
    }
  }
  // No reset time: nothing; a window that reset: "reset".
  const bare = limitsModel(claude({ windows: [{ id: "five_hour", label_ru: "", label_en: "", used_pct: 5, resets_at: "" }] }), "en", NOW, BALI);
  assert.equal(openReset(bare.rows[0], { open_reset: "in" }, WORDS.en, "en", NOW, false), "");
  const later = Date.parse("2026-10-28T10:30:00Z");
  const reset = limitsModel(claude(), "ru", later, BALI);
  assert.equal(openReset(reset.rows[0], { open_reset: "in" }, WORDS.ru, "ru", later, false), "сброшено");
});

test("open, compact: a line per window, the plan on the first, the age on the second", () => {
  const lines = compactLines(limitsModel(claude(), "ru", NOW, BALI), show(), WORDS.ru, "ru", NOW);
  assert.deepEqual(lines, [
    { id: "five_hour", label: "5ч", pct: 34, level: "ok", reset: "через 3ч", meta: [{ text: "Claude Max", blocked: false }] },
    { id: "seven_day", label: "неделя", pct: 61, level: "ok", reset: "через 5д", meta: [] },
  ]);
  // Left, the clock, refused, old.
  const m = limitsModel(claude({ blocked: true, fetched_at: "2026-10-28T07:20:00Z" }), "en", NOW, BALI);
  assert.deepEqual(
    compactLines(m, show({ percent: "left", open_reset: "clock" }), WORDS.en, "en", NOW).map((l) => [l.label, l.pct, l.level, l.reset, l.meta.map((x) => x.text)]),
    [
      ["5h", 66, "ok", "18:20", ["limit reached"]],
      ["week", 39, "high", "Mon 03:00", ["updated 40 min ago"]],
    ],
  );
  // One window: the age after the plan.
  const one = limitsModel(claude({ fetched_at: "2026-10-28T07:20:00Z", windows: [claude().windows[0]] }), "en", NOW, BALI);
  assert.deepEqual(compactLines(one, show(), WORDS.en, "en", NOW)[0].meta.map((x) => x.text), ["Claude Max", "updated 40 min ago"]);
});
