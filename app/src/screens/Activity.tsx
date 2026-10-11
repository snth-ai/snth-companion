import { useMemo, useState } from "react";
import { buildLines, dayLabel, formatClock, type LineKind } from "../lib/format";
import { useStore } from "../lib/store";
import { IconCookie, IconFolder, IconGlobe, IconLink, IconPointer } from "../ui/icons";

type Filter = "all" | "net" | "cookies" | "computer" | "files";

const ICONS: Record<LineKind, () => React.JSX.Element> = {
  net: IconGlobe,
  cookies: IconCookie,
  computer: IconPointer,
  files: IconFolder,
  link: IconLink,
};

export function Activity() {
  const { journal, s, lang, name } = useStore();
  const t = s.activity;
  const [filter, setFilter] = useState<Filter>("all");

  const lines = useMemo(() => {
    const entries = filter === "all" ? journal : journal.filter((e) => e.capability === filter);
    return buildLines(entries, s, lang);
  }, [journal, filter, s, lang]);

  // Newest first, with a heading where the day changes.
  const days: { label: string; lines: typeof lines }[] = [];
  for (const line of lines) {
    const label = dayLabel(line.time, s, lang);
    const last = days[days.length - 1];
    if (last && last.label === label) last.lines.push(line);
    else days.push({ label, lines: [line] });
  }

  const filters: { value: Filter; title: string }[] = [
    { value: "all", title: t.all },
    { value: "net", title: t.net },
    { value: "cookies", title: t.cookies },
    { value: "computer", title: t.computer },
    { value: "files", title: t.files },
  ];

  return (
    <>
      <h1 className="page-title">{s.nav.activity}</h1>
      <p className="page-lead">{t.lead(name)}</p>

      <div className="filters" role="group" aria-label={s.nav.activity}>
        {filters.map((f) => (
          <button key={f.value} type="button" className="chip" aria-pressed={filter === f.value} onClick={() => setFilter(f.value)}>
            {f.title}
          </button>
        ))}
      </div>

      {days.length === 0 ? (
        <div className="group" style={{ marginTop: 16 }}>
          <div className="empty">{filter === "all" ? t.empty : t.emptyFiltered}</div>
        </div>
      ) : (
        days.map((day) => (
          <div key={day.label}>
            <h2 className="day">{day.label}</h2>
            <ul className="group">
              {day.lines.map((line) => {
                const Icon = ICONS[line.kind];
                const note = [line.count > 1 ? t.times(line.count) : "", line.note].filter(Boolean).join(", ");
                return (
                  <li className={`line${line.refused ? " refused" : ""}`} key={line.key}>
                    <span className="line-time">{formatClock(line.time, lang)}</span>
                    <Icon />
                    <span className="line-text selectable">{line.text}</span>
                    {note ? <span className="line-note">{note}</span> : null}
                  </li>
                );
              })}
            </ul>
          </div>
        ))
      )}
    </>
  );
}
