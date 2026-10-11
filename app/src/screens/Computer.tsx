import type { ComputerMode, Foreground } from "../backend/types";
import { useStore } from "../lib/store";
import { ChoiceList, ListEditor, Notice, Row, Section, SwitchRow } from "../ui/controls";
import { HelperStatus } from "./HelperStatus";
import { Remembered } from "./Remembered";

const AWAY_MINUTES = [1, 2, 5, 10, 15, 30, 60];

export function Computer() {
  const { state, s, name, edit } = useStore();
  const c = state.core.settings.computer;
  const t = s.computer;
  const minutes = [...new Set([...AWAY_MINUTES, c.away_idle_minutes])].sort((a, b) => a - b);

  const awaySelect = (
    <>
      {t.awayAfter}
      <select
        className="select"
        aria-label={t.awayAfter}
        value={c.away_idle_minutes}
        onChange={(e) => edit((d) => void (d.computer.away_idle_minutes = Number(e.target.value)))}
      >
        {minutes.map((m) => (
          <option key={m} value={m}>
            {s.units.minutes(m)}
          </option>
        ))}
      </select>
    </>
  );

  return (
    <>
      <h1 className="page-title">{s.nav.computer}</h1>
      <p className="page-lead">{t.lead(name)}</p>

      <Section>
        <div className="group">
          <SwitchRow strong title={t.enable(name)} checked={c.enabled} onChange={(v) => edit((d) => void (d.computer.enabled = v))} />
        </div>
        <div style={{ marginTop: 8 }}>
          <Notice kind={c.enabled ? "warn" : "info"}>{t.reach(name)}</Notice>
        </div>
      </Section>

      <fieldset disabled={!c.enabled}>
        <Section title={t.modeTitle}>
          <ChoiceList<ComputerMode>
            label={t.modeTitle}
            value={c.mode}
            onChange={(v) => edit((d) => void (d.computer.mode = v))}
            options={[
              { value: "ask", title: t.ask, desc: t.askDesc(name) },
              { value: "auto", title: t.auto, desc: t.autoDesc(name) },
              { value: "away", title: t.away, desc: t.awayDesc(name), extra: awaySelect },
            ]}
          />
        </Section>

        <Section title={t.fgTitle} note={c.foreground === "idle" && c.mode !== "away" ? awaySelect : undefined}>
          <div className="group">
            <Row title={t.fgLead}>
              <select
                className="select"
                aria-label={t.fgTitle}
                value={c.foreground}
                onChange={(e) => edit((d) => void (d.computer.foreground = e.target.value as Foreground))}
              >
                <option value="never">{t.fgNever}</option>
                <option value="idle">{t.fgIdle}</option>
                <option value="ask">{t.fgAsk}</option>
                <option value="always">{t.fgAlways}</option>
              </select>
            </Row>
          </div>
        </Section>

        <Section title={t.blockedTitle} note={t.blockedDesc(name)}>
          <ListEditor
            items={c.app_blocklist.map((a) => ({ key: a, label: a }))}
            placeholder={t.blockedPlaceholder}
            addLabel={s.common.add}
            removeLabel={s.common.remove}
            emptyText={t.blockedEmpty}
            onAdd={(value) => {
              edit((d) => {
                if (!d.computer.app_blocklist.some((a) => a.toLowerCase() === value.toLowerCase())) d.computer.app_blocklist.push(value);
              });
              return null;
            }}
            onRemove={(key) => edit((d) => void (d.computer.app_blocklist = d.computer.app_blocklist.filter((a) => a !== key)))}
          />
        </Section>
      </fieldset>

      <Remembered capability="computer" />

      <Section title={t.needsTitle}>
        <HelperStatus live={false} />
      </Section>
    </>
  );
}
