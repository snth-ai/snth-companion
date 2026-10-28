// What the island shows of the synth's subscription limits
// (tz-island-limits.md), apart from React so it can be tested
// (node --test src/island/*.test.mjs).
//
// Open (hover): the plan's name and one row per window: the synth's
// label in the island's language, a bar, the percent used and when the
// window resets in this computer's time (today: "18:20"; another day:
// "Mon 03:00" / "пн 03:00"). Closed: a dot when any window is at 80% or
// more (yellow) or 95% or more, or the provider already refuses (red).
//
// The snapshot may be old: a window whose reset time has passed shows as
// 0% "reset" (never the old percent), and a snapshot older than 30
// minutes says how old it is.
//
// How they show is the owner's choice (`LimitsDisplay`,
// tz-island-limits-display.md). Closed, by default a short text beside
// the status: a window's letter, the percent and the time to its reset
// ("h:34% 2h", "w:61% 3d"; `closedParts`), the same tokens in both
// languages. Open, by default a line per window (`LimitsRows`); the
// block above is "detailed".

import type { IslandLimits, IslandLimitWindow, LimitsDisplay } from "../backend/types";

/** At or above: yellow. */
export const WARN_PCT = 80;
/** At or above (or the provider refuses): red. */
export const HIGH_PCT = 95;
/** Older than this, the island says when it was read. */
export const STALE_MS = 30 * 60_000;

export type LimitLevel = "ok" | "warn" | "high";

export interface LimitRow {
  id: string;
  label: string;
  /** The closed text's token: "h", "w", "wO" (`windowLetter`). */
  letter: string;
  /** Rounded, 0..100 (0 once the window has reset). */
  pct: number;
  level: LimitLevel;
  /** "18:20", "Mon 03:00", "" (unknown or reset). */
  reset: string;
  /** When it resets (unix ms); null when unknown or already reset. */
  resetAt: number | null;
  /** The reset time has passed since the snapshot. */
  wasReset: boolean;
}

export interface LimitsModel {
  title: string;
  rows: LimitRow[];
  /** The provider refuses right now (and no window reset since). */
  blocked: boolean;
  /** The dot on the closed island: null = none. */
  dot: Exclude<LimitLevel, "ok"> | null;
  /** Minutes since the snapshot was read, when older than `STALE_MS`. */
  ageMin: number | null;
}

const DEFAULT_LABELS: Record<string, { ru: string; en: string }> = {
  five_hour: { ru: "5ч", en: "5h" },
  seven_day: { ru: "неделя", en: "week" },
};

/** The plan's name, or the provider's when the synth sent none. */
export function planTitle(l: Pick<IslandLimits, "plan" | "provider">): string {
  const plan = (l.plan ?? "").trim();
  if (plan) return plan;
  if (l.provider === "claude") return "Claude";
  if (l.provider === "codex") return "ChatGPT";
  return (l.provider ?? "").trim();
}

/** The window's label in this language, the other one, or a default. */
export function windowLabel(w: Pick<IslandLimitWindow, "id" | "label_ru" | "label_en">, lang: "en" | "ru"): string {
  const own = (lang === "ru" ? w.label_ru : w.label_en)?.trim();
  if (own) return own;
  const other = (lang === "ru" ? w.label_en : w.label_ru)?.trim();
  if (other) return other;
  const d = DEFAULT_LABELS[w.id];
  return d ? d[lang] : w.id;
}

/** The closed text's token for a window: "h" (5 hours), "w" (the week),
 * "w" and the model's initial for a model's own week ("wO" for "week
 * Opus"), else the id's first two letters. */
export function windowLetter(w: Pick<IslandLimitWindow, "id" | "label_ru" | "label_en">): string {
  if (w.id === "five_hour") return "h";
  if (w.id === "seven_day") return "w";
  if (w.id.startsWith("seven_day_")) {
    const words = `${w.label_en ?? ""} ${w.label_ru ?? ""}`.split(/\s+/).filter((x) => x && !/^(week|weekly|7d|неделя|недели|7д)$/i.test(x));
    const model = words[0] ?? w.id.slice("seven_day_".length);
    return model ? `w${model[0]!.toUpperCase()}` : "w";
  }
  return w.id.slice(0, 2);
}

/** The time to a reset, short and rounded up: "45m" under an hour, "2h"
 * under a day, "3d" from a day on (the closed text's units, the same in
 * both languages). "" when it is not ahead. */
export function untilShort(ms: number): string {
  return untilIn(ms, { m: "m", h: "h", d: "d" });
}

/** The same in the island's language ("45м", "2ч", "3д" in Russian). */
export function untilText(ms: number, lang: "en" | "ru"): string {
  return untilIn(ms, lang === "ru" ? { m: "м", h: "ч", d: "д" } : { m: "m", h: "h", d: "d" });
}

function untilIn(ms: number, u: { m: string; h: string; d: string }): string {
  if (!Number.isFinite(ms) || ms <= 0) return "";
  const min = Math.ceil(ms / 60_000);
  if (min < 60) return `${min}${u.m}`;
  if (ms < 24 * 3_600_000) return `${Math.ceil(ms / 3_600_000)}${u.h}`;
  return `${Math.ceil(ms / 86_400_000)}${u.d}`;
}

export function levelOf(pct: number): LimitLevel {
  return pct >= HIGH_PCT ? "high" : pct >= WARN_PCT ? "warn" : "ok";
}

/** The calendar day of `ms` in `timeZone` (undefined: this computer's). */
function dayKey(ms: number, timeZone?: string): string {
  return new Intl.DateTimeFormat("en-CA", { timeZone, year: "numeric", month: "2-digit", day: "2-digit" }).format(ms);
}

/** When a window resets, in this computer's time: today "18:20",
 * within the week "Mon 03:00" / "пн 03:00", later "4 Nov 03:00". "" for
 * a time that does not parse. */
export function resetText(iso: string, now: number, lang: "en" | "ru", timeZone?: string): string {
  const at = Date.parse(iso);
  if (!iso || Number.isNaN(at)) return "";
  const locale = lang === "ru" ? "ru-RU" : "en-GB";
  const time = new Intl.DateTimeFormat(locale, { timeZone, hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(at);
  if (dayKey(at, timeZone) === dayKey(now, timeZone)) return time;
  if (at - now < 6 * 24 * 3_600_000) {
    const day = new Intl.DateTimeFormat(locale, { timeZone, weekday: "short" }).format(at).replace(/\.$/, "");
    return `${day} ${time}`;
  }
  const date = new Intl.DateTimeFormat(locale, { timeZone, day: "numeric", month: "short" }).format(at).replace(/\.$/, "");
  return `${date} ${time}`;
}

/** When the snapshot was read: the synth's `fetched_at`, or the receipt. */
function readAt(l: IslandLimits): number {
  const f = Date.parse(l.fetched_at ?? "");
  return Number.isNaN(f) ? (l.received_ms ?? 0) : f;
}

/** The island's model of a snapshot, or null when there is nothing to
 * show (no snapshot, or neither a window nor a refusal in it). */
export function limitsModel(l: IslandLimits | null | undefined, lang: "en" | "ru", now: number, timeZone?: string): LimitsModel | null {
  if (!l) return null;
  const windows = Array.isArray(l.windows) ? l.windows.filter((w) => typeof w.used_pct === "number" && Number.isFinite(w.used_pct)) : [];
  const rows: LimitRow[] = windows.map((w) => {
    const at = Date.parse(w.resets_at ?? "");
    const wasReset = !Number.isNaN(at) && at <= now;
    const pct = wasReset ? 0 : Math.round(Math.min(100, Math.max(0, w.used_pct)));
    return {
      id: w.id,
      label: windowLabel(w, lang),
      letter: windowLetter(w),
      pct,
      level: levelOf(pct),
      reset: wasReset ? "" : resetText(w.resets_at ?? "", now, lang, timeZone),
      resetAt: wasReset || Number.isNaN(at) ? null : at,
      wasReset,
    };
  });
  // A refusal ends with the window that caused it: once any window has
  // reset since, the snapshot's refusal is not trusted any more.
  const blocked = Boolean(l.blocked) && !rows.some((r) => r.wasReset);
  if (blocked && rows.length > 0) {
    const top = rows.reduce((a, b) => (b.pct > a.pct ? b : a));
    top.level = "high";
  }
  if (rows.length === 0 && !blocked) return null;
  const dot = blocked || rows.some((r) => r.level === "high") ? "high" : rows.some((r) => r.level === "warn") ? "warn" : null;
  const read = readAt(l);
  const age = read > 0 ? now - read : 0;
  return {
    title: planTitle(l),
    rows,
    blocked,
    dot,
    ageMin: age > STALE_MS ? Math.floor(age / 60_000) : null,
  };
}

// --- the owner's choice of what shows (tz-island-limits-display.md) ---

export const DEFAULT_DISPLAY: LimitsDisplay = {
  closed: "text",
  window: "auto",
  percent: "used",
  closed_reset: "in",
  open: "compact",
  open_reset: "in",
};

const CHOICES: { [K in keyof LimitsDisplay]: readonly LimitsDisplay[K][] } = {
  closed: ["text", "dot", "off"],
  window: ["auto", "five_hour", "week", "both"],
  percent: ["used", "left"],
  closed_reset: ["in", "off"],
  open: ["compact", "full", "off"],
  open_reset: ["in", "clock"],
};

/** The settings as the shell sent them, every field known: a missing or
 * unknown one is the default. */
export function displayOf(d: Partial<LimitsDisplay> | null | undefined): LimitsDisplay {
  const out = { ...DEFAULT_DISPLAY };
  if (!d) return out;
  for (const key of Object.keys(CHOICES) as (keyof LimitsDisplay)[]) {
    const v = d[key];
    if (v !== undefined && (CHOICES[key] as readonly string[]).includes(v)) (out as Record<string, string>)[key] = v;
  }
  return out;
}

/** The percent as the owner reads it: used, or left (100 - used). */
export function shownPct(pct: number, d: Pick<LimitsDisplay, "percent">): number {
  return d.percent === "left" ? 100 - pct : pct;
}

/** The window a choice names, or null. */
function pick(m: LimitsModel, id: string): LimitRow | null {
  return m.rows.find((r) => r.id === id) ?? null;
}

/** The fullest window; on a tie the 5-hour one, then the first. */
export function fullest(m: LimitsModel): LimitRow | null {
  let best: LimitRow | null = null;
  for (const r of m.rows) {
    if (!best || r.pct > best.pct || (r.pct === best.pct && r.id === "five_hour" && best.id !== "five_hour")) best = r;
  }
  return best;
}

/** The windows the closed text shows: the chosen one (or both), the
 * fullest when the snapshot has no such window. */
export function closedRows(m: LimitsModel, window: LimitsDisplay["window"]): LimitRow[] {
  const auto = fullest(m);
  const chosen =
    window === "five_hour"
      ? [pick(m, "five_hour")]
      : window === "week"
        ? [pick(m, "seven_day")]
        : window === "both"
          ? [pick(m, "five_hour"), pick(m, "seven_day")]
          : [auto];
  const rows = chosen.filter((r): r is LimitRow => r !== null);
  return rows.length > 0 ? rows : auto ? [auto] : [];
}

export interface ClosedPart {
  /** For React. */
  key: string;
  /** "h:34% 2h". */
  text: string;
  /** The colour: the window's level, red for all while refused. */
  level: LimitLevel;
}

/** The closed island's text, a part per window: "h:34% 2h", "w:61% 3d";
 * a window that reset "h:0%" (no countdown). Refused with no window at
 * all: `blockedWord` in red. Empty: nothing to show. */
export function closedParts(m: LimitsModel, d: LimitsDisplay, now: number, blockedWord: string): ClosedPart[] {
  const rows = closedRows(m, d.window);
  if (rows.length === 0) return m.blocked ? [{ key: "blocked", text: blockedWord, level: "high" }] : [];
  return rows.map((r) => {
    const until = d.closed_reset === "in" && r.resetAt !== null ? untilShort(r.resetAt - now) : "";
    return {
      key: r.id,
      text: `${r.letter}:${shownPct(r.pct, d)}%${until ? ` ${until}` : ""}`,
      level: m.blocked ? "high" : r.level,
    };
  });
}

/** What the closed text may fall back to when it does not fit (the notch
 * has room for so much): the owner's choice first, then one window, then
 * that window without its countdown. */
export function closedFallbacks(d: LimitsDisplay): LimitsDisplay[] {
  const out: LimitsDisplay[] = [d];
  if (d.window === "both") out.push({ ...d, window: "auto" });
  if (d.closed_reset === "in") out.push({ ...out[out.length - 1]!, closed_reset: "off" });
  return out;
}

/** The words the open island's reset needs (the island's strings). */
export interface ResetWords {
  limitsWasReset: string;
  limitsIn: (left: string) => string;
  limitsResetsIn: (left: string) => string;
  limitsResets: (when: string) => string;
}

/** A window's reset on the open island, as the owner chose: the time
 * left ("in 2h", "через 2ч") or the clock ("18:20", "пн 03:00"); `long`
 * says "resets" too (the detailed block's wide card). A window that
 * reset: "reset". "" when unknown. */
export function openReset(r: LimitRow, d: Pick<LimitsDisplay, "open_reset">, w: ResetWords, lang: "en" | "ru", now: number, long: boolean): string {
  if (r.wasReset) return w.limitsWasReset;
  if (d.open_reset === "in" && r.resetAt !== null) {
    const left = untilText(r.resetAt - now, lang);
    if (left) return long ? w.limitsResetsIn(left) : w.limitsIn(left);
  }
  if (!r.reset) return "";
  return long ? w.limitsResets(r.reset) : r.reset;
}

export interface CompactLine {
  id: string;
  label: string;
  /** As the owner reads it (used or left), and the bar's width. */
  pct: number;
  level: LimitLevel;
  reset: string;
  /** At the line's far end: the plan (or "limit reached") on the first
   * line, the snapshot's age on the second (after the plan when there
   * is one line). */
  meta: { text: string; blocked: boolean }[];
}

/** The compact open view: a line per window. */
export function compactLines(
  m: LimitsModel,
  d: LimitsDisplay,
  w: ResetWords & { limitsBlocked: string; limitsAge: (min: number) => string },
  lang: "en" | "ru",
  now: number,
): CompactLine[] {
  const plan = m.blocked ? { text: w.limitsBlocked, blocked: true } : { text: m.title, blocked: false };
  const age = m.ageMin !== null ? { text: w.limitsAge(m.ageMin), blocked: false } : null;
  return m.rows.map((r, i) => {
    const meta = i === 0 ? [plan, ...(m.rows.length === 1 && age ? [age] : [])] : i === 1 && age ? [age] : [];
    return { id: r.id, label: r.label, pct: shownPct(r.pct, d), level: r.level, reset: openReset(r, d, w, lang, now, false), meta };
  });
}
