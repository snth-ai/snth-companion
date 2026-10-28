// The synth's subscription limits on the island (tz-island-limits.md,
// tz-island-limits-display.md): the open island's compact lines
// (`LimitsRows`, the default) or detailed block (`LimitsBlock`), and the
// closed island's short text (`LimitsText`, the default) or dot
// (`LimitsDot`). The numbers and the words come from limitsView.ts; the
// owner's choices arrive as `LimitsDisplay`. The Island screen's preview
// draws the same parts.

import type { LimitsDisplay } from "../backend/types";
import type { stringsFor } from "../i18n";
import type { ClosedPart, LimitsModel } from "./limitsView";
import { compactLines, openReset, shownPct } from "./limitsView";
import "./limits.css";

type IslandWords = ReturnType<typeof stringsFor>["island"]["w"];

/** One line for a tooltip or a screen reader: "Claude Max: 5h 34%
 * (resets 18:20), week 61% (resets Mon 03:00)". */
export function limitsSummary(m: LimitsModel, t: IslandWords): string {
  const rows = m.rows
    .map((r) => {
      const when = r.wasReset ? t.limitsWasReset : r.reset ? t.limitsResets(r.reset) : "";
      return `${r.label} ${r.pct}%${when ? ` (${when})` : ""}`;
    })
    .join(", ");
  const head = rows ? `${m.title}: ${rows}` : m.title;
  const blocked = m.blocked ? ` (${t.limitsBlocked})` : "";
  const age = m.ageMin !== null ? `. ${t.limitsAge(m.ageMin)}` : "";
  return `${head}${blocked}${age}`;
}

interface OpenProps {
  model: LimitsModel;
  t: IslandWords;
  display: LimitsDisplay;
  lang: "en" | "ru";
  now: number;
  /** A narrow card (the middle of a side, beside the face's column). */
  compact?: boolean;
}

/** The detailed block (0.1.33): the plan, then a row per window with a
 * bar. In a narrow card the reset goes without its word. */
export function LimitsBlock(props: OpenProps) {
  const { model: m, t, display: d, lang, now, compact } = props;
  return (
    <div className={`isl-limits${compact ? " compact" : ""}`} role="group" aria-label={t.limits}>
      <div className="isl-limits-head">
        <span className="isl-limits-plan">{m.title}</span>
        {m.blocked ? <span className="isl-limits-blocked">{t.limitsBlocked}</span> : null}
        {m.ageMin !== null ? <span className="isl-limits-age">{t.limitsAge(m.ageMin)}</span> : null}
      </div>
      {m.rows.length > 0 ? (
        <div className="isl-limits-rows">
          {m.rows.map((r) => {
            const pct = shownPct(r.pct, d);
            return (
              <div key={r.id} className={`isl-limits-row level-${r.level}`}>
                <span className="isl-limits-label" title={r.label}>
                  {r.label}
                </span>
                <span className="isl-limits-bar" aria-hidden="true">
                  <i style={{ width: `${pct}%` }} />
                </span>
                <span className="isl-limits-pct">{pct}%</span>
                <span className="isl-limits-reset">{openReset(r, d, t, lang, now, !compact)}</span>
              </div>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

/** The compact lines (the default): a line per window, small, a thin
 * bar, the percent and the reset; the plan (or "limit reached") at the
 * end of the first line and the snapshot's age at the end of the second,
 * dimmed. The whole summary is the tooltip. */
export function LimitsRows(props: OpenProps) {
  const { model: m, t, display: d, lang, now } = props;
  const summary = limitsSummary(m, t);
  const metaOf = (meta: { text: string; blocked: boolean }[]) =>
    meta.map((x) => (
      <span key={x.text} className={`isl-lc-meta${x.blocked ? " blocked" : ""}`}>
        {x.text}
      </span>
    ));
  if (m.rows.length === 0) {
    // Refused with no window: the plan and "limit reached" alone.
    const only = [{ text: m.title, blocked: false }, ...(m.blocked ? [{ text: t.limitsBlocked, blocked: true }] : [])];
    return (
      <div className="isl-lc" role="group" aria-label={summary} title={summary}>
        <div className="isl-lc-only">{metaOf(only)}</div>
      </div>
    );
  }
  return (
    <div className="isl-lc" role="group" aria-label={summary} title={summary}>
      {compactLines(m, d, t, lang, now).map((l) => (
        <div key={l.id} className={`isl-lc-row level-${l.level}`}>
          <span className="isl-lc-label">{l.label}</span>
          <span className="isl-lc-bar" aria-hidden="true">
            <i style={{ width: `${l.pct}%` }} />
          </span>
          <span className="isl-lc-pct">{l.pct}%</span>
          <span className="isl-lc-reset">{l.reset}</span>
          {l.meta.length > 1 ? <span className="isl-lc-metas">{metaOf(l.meta)}</span> : l.meta.length === 1 ? metaOf(l.meta) : <span />}
        </div>
      ))}
    </div>
  );
}

/** The closed island's text: "h:34% 2h", or "h:34% 2h  w:61% 3d", each
 * part in its window's colour. The summary is its tooltip. */
export function LimitsText(props: { parts: ClosedPart[]; label: string }) {
  if (props.parts.length === 0) return null;
  return (
    <span className="isl-lt" title={props.label} aria-label={props.label} role="img">
      {props.parts.map((p) => (
        <span key={p.key} className={`isl-lt-part level-${p.level}`}>
          {p.text}
        </span>
      ))}
    </span>
  );
}

/** The closed island's dot: yellow from 80%, red from 95% or refused. */
export function LimitsDot(props: { model: LimitsModel; t: IslandWords }) {
  const { model: m, t } = props;
  if (!m.dot) return null;
  const label = limitsSummary(m, t);
  return <span className={`isl-limits-dot ${m.dot}`} title={label} aria-label={label} role="img" />;
}
