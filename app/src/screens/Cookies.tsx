import { backend } from "../backend";
import type { CookiesMode } from "../backend/types";
import { grantTime } from "../lib/format";
import { useStore } from "../lib/store";
import { ChoiceList, More, Notice, Section, SwitchRow } from "../ui/controls";

export function Cookies() {
  const { state, s, lang, name, edit, act } = useStore();
  const c = state.core.settings.cookies;
  const t = s.cookies;
  const grants = state.grants.filter((g) => g.capability === "cookies").sort((a, b) => a.scope.localeCompare(b.scope));

  return (
    <>
      <h1 className="page-title">{s.nav.cookies}</h1>
      <p className="page-lead">{t.lead(name)}</p>

      <Section>
        <div className="group">
          <SwitchRow strong title={t.enable(name)} checked={c.enabled} onChange={(v) => edit((d) => void (d.cookies.enabled = v))} />
        </div>
        <div style={{ marginTop: 8 }}>
          <Notice kind={c.enabled ? "warn" : "info"}>{t.warning(name)}</Notice>
        </div>
      </Section>

      <fieldset disabled={!c.enabled}>
        <Section title={t.modeTitle(name)}>
          <ChoiceList<CookiesMode>
            label={t.modeTitle(name)}
            value={c.mode}
            onChange={(v) => edit((d) => void (d.cookies.mode = v))}
            options={[
              { value: "ask", title: t.ask, desc: t.askDesc },
              { value: "ask_new", title: t.askNew, desc: t.askNewDesc(name) },
              { value: "auto", title: t.auto, desc: t.autoDesc(name) },
            ]}
          />
        </Section>
      </fieldset>

      <Section
        title={t.listTitle}
        note={grants.length > 0 ? (c.mode === "ask" ? `${t.listUnused} ${t.revokeNote(name)}` : t.revokeNote(name)) : undefined}
      >
        <div className="group">
          {grants.length === 0 ? (
            <div className="empty">{c.mode === "ask" ? t.emptyAskMode : c.mode === "auto" ? t.emptyAuto(name) : t.empty(name)}</div>
          ) : (
            grants.map((g) => (
              <div className="item" key={g.scope}>
                <span className="item-text selectable">{g.scope}</span>
                <span className="item-sub">{grantTime(g, s, lang)}</span>
                <button type="button" className="btn small" onClick={() => void act(() => backend.revokeGrant("cookies", g.scope))}>
                  {t.revoke}
                </button>
              </div>
            ))
          )}
        </div>
      </Section>

      <More label={s.common.details}>
        <div>{state.app.os === "windows" ? t.browsersWindows : t.browsers}</div>
        {state.app.os === "macos" ? <div style={{ marginTop: 4 }}>{t.keychainNote}</div> : null}
      </More>
    </>
  );
}
