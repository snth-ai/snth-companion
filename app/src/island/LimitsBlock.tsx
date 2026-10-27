// The synth's subscription limits on the island (tz-island-limits.md):
// the block in the open island (hover) and the dot on the closed one.
// The numbers and the words come from `limitsModel` (limitsView.ts).

import type { stringsFor } from "../i18n";
import type { LimitsModel } from "./limitsView";

type IslandWords = ReturnType<typeof stringsFor>["island"]["w"];

/** One line for a tooltip or a screen reader: "Claude Max: 5h 85%, week 61%". */
export function limitsSummary(m: LimitsModel, t: IslandWords): string {
  const rows = m.rows.map((r) => `${r.label} ${r.pct}%`).join(", ");
  const head = rows ? `${m.title}: ${rows}` : m.title;
  return m.blocked ? `${head} (${t.limitsBlocked})` : head;
}

/** The open island's block: the plan, then a row per window. `compact`
 * (a narrow card: the middle of a side, beside the face's column): the
 * reset time without its word ("Sun 12:15", not "resets Sun 12:15"). */
export function LimitsBlock(props: { model: LimitsModel; t: IslandWords; compact?: boolean }) {
  const { model: m, t, compact } = props;
  const resets = (when: string) => (compact ? when : t.limitsResets(when));
  return (
    <div className={`isl-limits${compact ? " compact" : ""}`} role="group" aria-label={t.limits}>
      <div className="isl-limits-head">
        <span className="isl-limits-plan">{m.title}</span>
        {m.blocked ? <span className="isl-limits-blocked">{t.limitsBlocked}</span> : null}
        {m.ageMin !== null ? <span className="isl-limits-age">{t.limitsAge(m.ageMin)}</span> : null}
      </div>
      {m.rows.length > 0 ? (
        <div className="isl-limits-rows">
          {m.rows.map((r) => (
            <div key={r.id} className={`isl-limits-row level-${r.level}`}>
              <span className="isl-limits-label" title={r.label}>
                {r.label}
              </span>
              <span className="isl-limits-bar" aria-hidden="true">
                <i style={{ width: `${r.pct}%` }} />
              </span>
              <span className="isl-limits-pct">{r.pct}%</span>
              <span className="isl-limits-reset">{r.wasReset ? t.limitsWasReset : r.reset ? resets(r.reset) : ""}</span>
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/** The closed island's dot: yellow from 80%, red from 95% or refused. */
export function LimitsDot(props: { model: LimitsModel; t: IslandWords }) {
  const { model: m, t } = props;
  if (!m.dot) return null;
  const label = limitsSummary(m, t);
  return <span className={`isl-limits-dot ${m.dot}`} title={label} aria-label={label} role="img" />;
}
