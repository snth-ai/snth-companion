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

import type { IslandLimits, IslandLimitWindow } from "../backend/types";

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
  /** Rounded, 0..100 (0 once the window has reset). */
  pct: number;
  level: LimitLevel;
  /** "18:20", "Mon 03:00", "" (unknown or reset). */
  reset: string;
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
      pct,
      level: levelOf(pct),
      reset: wasReset ? "" : resetText(w.resets_at ?? "", now, lang, timeZone),
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
