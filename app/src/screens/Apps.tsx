// The owner's own apps: one master switch, a switch per app with a plain
// line about what the synth can do there, the state of the system
// permission each app needs, and when to ask before a change.
//
// Only the apps this computer has are listed: `state.apps` comes from the
// core, which offers no Mac app on Windows.

import { useCallback, useEffect, useState } from "react";
import { backend } from "../backend";
import type { AppPermission, AppStatus, AppsMode, Grant } from "../backend/types";
import { grantTime } from "../lib/format";
import { grantSynth, useStore } from "../lib/store";
import type { Strings } from "../i18n";
import { ChoiceList, Notice, Section, Switch, SwitchRow } from "../ui/controls";
import { IconCheck, IconWarning } from "../ui/icons";

/** The permission states, kept current: re-read when the settings change
 * and, for a while after "ask now", on a timer (the answer to the
 * system's own question arrives without any event). */
function useAppsStatus(watch: boolean) {
  const { state } = useStore();
  const [status, setStatus] = useState<AppStatus[] | null>(null);
  const [checking, setChecking] = useState(false);
  const rev = state.core.settings_rev;

  const check = useCallback(async () => {
    setChecking(true);
    try {
      setStatus((await backend.appsCheck()).apps);
    } catch {
      /* keep the last answer */
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    void check();
  }, [check, rev]);

  useEffect(() => {
    if (!watch) return;
    const id = window.setInterval(() => void check(), 2500);
    return () => window.clearInterval(id);
  }, [check, watch]);

  return { status, checking, check };
}

function PermissionLine(props: { app: string; title: string; permission: AppPermission; onAsk: () => void }) {
  const { s } = useStore();
  const t = s.apps;
  const { kind, state } = props.permission;

  if (kind === "full_disk") {
    if (state === "granted") {
      return (
        <div className="app-perm yes">
          <IconCheck />
          <span>{t.diskAllowed}</span>
        </div>
      );
    }
    if (state === "unknown") {
      return (
        <div className="app-perm">
          <span>{t.diskUnknown}</span>
        </div>
      );
    }
    return (
      <div className="app-perm no">
        <IconWarning />
        <span>
          <b>{t.diskDenied}.</b> {t.diskHow}
        </span>
        <button type="button" className="btn small" onClick={() => void backend.openExternal("privacy_full_disk")}>
          {t.openSettings}
        </button>
      </div>
    );
  }

  // Automation: may SNTH Connect send commands to this app?
  if (state === "granted") {
    return (
      <div className="app-perm yes">
        <IconCheck />
        <span>{t.permAllowed}</span>
      </div>
    );
  }
  if (state === "denied") {
    return (
      <div className="app-perm no">
        <IconWarning />
        <span>{t.permDenied(props.title)}</span>
        <button type="button" className="btn small" onClick={() => void backend.openExternal("privacy_automation")}>
          {t.openSettings}
        </button>
      </div>
    );
  }
  return (
    <div className="app-perm">
      <span>{state === "not_asked" ? t.permNotAsked : t.permUnknown}</span>
      <button type="button" className="btn small" title={t.permAskHint(props.title)} onClick={props.onAsk}>
        {t.permAsk}
      </button>
    </div>
  );
}

function rememberedLabel(g: Grant, s: Strings): string {
  const t = s.apps;
  const [kind, ...rest] = g.scope.split(":");
  const what = rest.join(":");
  if (kind === "messages.send") return t.rememberedSend(what);
  if (kind === "shortcuts.run") return t.rememberedShortcut(what);
  if (kind === "open.url") return t.rememberedUrl(what);
  if (kind === "open.app") return t.rememberedApp(what);
  return t.remembered[g.scope] ?? g.scope;
}

export function Apps() {
  const { state, s, lang, name, edit, act } = useStore();
  const a = state.core.settings.apps;
  const t = s.apps;
  const mac = state.app.os === "macos";
  const [watch, setWatch] = useState(false);
  const { status, checking, check } = useAppsStatus(watch);

  useEffect(() => {
    if (!watch) return;
    const id = window.setTimeout(() => setWatch(false), 60_000);
    return () => window.clearTimeout(id);
  }, [watch]);

  const grants = state.grants.filter((g) => g.capability === "apps").sort((x, y) => x.scope.localeCompare(y.scope));
  const anyPermission = status?.some((app) => app.on && app.permissions.length > 0) ?? false;

  return (
    <>
      <h1 className="page-title">{s.nav.apps}</h1>
      <p className="page-lead">{t.lead(name)}</p>

      <Section>
        <div className="group">
          <SwitchRow strong title={t.enable(name)} checked={a.enabled} onChange={(v) => edit((d) => void (d.apps.enabled = v))} />
        </div>
        <div style={{ marginTop: 8 }}>
          <Notice kind={a.enabled ? "warn" : "info"}>{t.warning(name)}</Notice>
        </div>
      </Section>

      <fieldset disabled={!a.enabled}>
        <Section title={t.listTitle} note={mac ? t.listNoteMac : t.listNoteOther}>
          <div className="group">
            {state.apps.map((app) => {
              const title = t.title[app.id] ?? app.id;
              const permissions = a.enabled && app.on ? (status?.find((x) => x.id === app.id)?.permissions ?? []) : [];
              return (
                <div className="row app-row" key={app.id}>
                  <div className="row-text">
                    <div className="row-title">{title}</div>
                    <div className="row-desc">{t.desc[app.id] ?? ""}</div>
                    {permissions.map((p) => (
                      <PermissionLine
                        key={p.kind}
                        app={app.id}
                        title={title}
                        permission={p}
                        onAsk={() => {
                          setWatch(true);
                          void backend.appsRequestPermission(app.id).catch(() => undefined);
                        }}
                      />
                    ))}
                  </div>
                  <Switch
                    label={title}
                    checked={app.on}
                    onChange={(v) =>
                      edit((d) => {
                        d.apps.apps[app.id] = v;
                      })
                    }
                  />
                </div>
              );
            })}
          </div>
          {mac && a.enabled && anyPermission ? (
            <div className="onb-actions">
              <button type="button" className="btn small" disabled={checking} onClick={() => void check()}>
                {t.checkAgain}
              </button>
            </div>
          ) : null}
        </Section>

        <Section title={t.modeTitle(name)} note={t.modeNote}>
          <ChoiceList<AppsMode>
            label={t.modeTitle(name)}
            value={a.mode}
            onChange={(v) => edit((d) => void (d.apps.mode = v))}
            options={[
              { value: "ask", title: t.ask, desc: t.askDesc },
              { value: "ask_new", title: t.askNew, desc: t.askNewDesc(name) },
              { value: "auto", title: t.auto, desc: t.autoDesc(name) },
            ]}
          />
        </Section>
      </fieldset>

      {grants.length > 0 ? (
        <Section title={s.remembered.title} note={a.mode === "ask" ? t.rememberedUnused : undefined}>
          <div className="group">
            {grants.map((g) => (
              <div className="item" key={`${g.synth_id}/${g.scope}`}>
                <span className="item-text selectable">{rememberedLabel(g, s)}</span>
                {grantSynth(state, g) ? <span className="tag">{grantSynth(state, g)}</span> : null}
                <span className="item-sub">{grantTime(g, s, lang)}</span>
                <button type="button" className="btn small" onClick={() => void act(() => backend.revokeGrant("apps", g.scope, g.synth_id))}>
                  {s.cookies.revoke}
                </button>
              </div>
            ))}
          </div>
        </Section>
      ) : null}
    </>
  );
}
