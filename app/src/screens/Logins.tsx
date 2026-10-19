// Logins: everything the synth holds or may use to be signed in as the
// owner, site by site, and the browsers that can hand it over.
//
// Two kinds of login that never mix (the rule the page states at the top):
//   a password   shared into the synth's vault (the SNTH Vault extension,
//                the web form), used only in the synth's cloud browser;
//   a sign-in    a site the owner allowed here; its cookies are used only
//                for requests sent from this computer.
//
// The page never holds either: the vault list is site names and dates
// (asked of the synth when the page opens), a sign-in is a remembered
// permission for a site.

import { useCallback, useEffect, useState } from "react";
import { backend } from "../backend";
import type { BrowserRow, CookiesMode, Grant, VaultSite } from "../backend/types";
import { formatDay, grantTime } from "../lib/format";
import { grantSynth, linkName, useStore } from "../lib/store";
import { ChoiceList, More, Notice, Section, Segmented, Sheet, SwitchRow } from "../ui/controls";
import { IconCheck } from "../ui/icons";

interface SiteRow {
  site: string;
  password: VaultSite | null;
  /** One per synth the owner allowed the site for. */
  sessions: Grant[];
}

/** One row per site, from both sides. */
function bySite(passwords: VaultSite[], sessions: Grant[]): SiteRow[] {
  const rows = new Map<string, SiteRow>();
  for (const p of passwords) rows.set(p.site, { site: p.site, password: p, sessions: [] });
  for (const g of sessions) {
    const row = rows.get(g.scope);
    if (row) row.sessions.push(g);
    else rows.set(g.scope, { site: g.scope, password: null, sessions: [g] });
  }
  return [...rows.values()].sort((a, b) => a.site.localeCompare(b.site));
}

export function Logins() {
  const { state, s, lang, name: firstName, edit, act, set } = useStore();
  const c = state.core.settings.cookies;
  const t = s.logins;
  const { vault, browsers, link_ready, vault_synth } = state.logins;
  const links = state.core.links;
  const several = links.length > 1;
  // The vault on show is one synth's: its name goes into the sentences
  // about the vault.
  const vaultLink = links.find((l) => l.synth_id === vault_synth) ?? links[0];
  const name = vaultLink ? linkName(vaultLink) : firstName;
  const [choosing, setChoosing] = useState<BrowserRow | null>(null);
  const [chosen, setChosen] = useState<string>("");
  const [asking, setAsking] = useState(false);
  const [removing, setRemoving] = useState<string | null>(null);
  const [unlinking, setUnlinking] = useState<BrowserRow | null>(null);
  const [linking, setLinking] = useState<string | null>(null);
  const [linkError, setLinkError] = useState<{ install: string; code: string } | null>(null);
  const linked = vaultLink?.state === "linked";

  // What the vault holds is asked of the synth when the page opens, and
  // again when the link comes back.
  const refresh = useCallback(async (synthId: string | null = null) => {
    setAsking(true);
    try {
      set(await backend.loginsRefresh(synthId));
    } catch {
      /* the last answer stays */
    } finally {
      setAsking(false);
    }
  }, [set]);

  useEffect(() => {
    void refresh(vault_synth);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [refresh, linked]);

  // Safari's extension ships inside the app, and only the owner can
  // switch it on (in Safari's settings). While that is still to do, Safari
  // is asked now and then, and when the window comes back, so the row
  // follows them through it without a button to press.
  const safariPending = browsers.some((b) => b.browser === "safari" && b.state === "missing" && b.safari?.enabled !== "translocated");
  useEffect(() => {
    if (!safariPending) return;
    const check = () => {
      if (document.visibilityState !== "visible") return;
      backend.safariCheck().then(set, () => undefined);
    };
    const timer = window.setInterval(check, 3000);
    window.addEventListener("focus", check);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", check);
    };
  }, [safariPending, set]);

  const sessions = state.grants.filter((g) => g.capability === "cookies");
  const rows = bySite(vault.state === "ready" ? vault.sites : [], sessions);

  const vaultNote =
    vault.state === "ready"
      ? null
      : vault.state === "unknown"
        ? { text: t.vaultAsking(name), retry: false }
        : vault.state === "offline"
          ? { text: t.vaultOffline(name), retry: true }
          : vault.state === "outdated"
            ? { text: t.vaultOutdated(name), retry: false }
            : vault.state === "unavailable"
              ? { text: t.vaultUnavailable(name), retry: false }
              : { text: t.vaultFailed, retry: true };

  const link = async (install: string, synthId: string | null) => {
    setLinking(install);
    setLinkError(null);
    try {
      set(await backend.browserLink(install, synthId));
      void refresh(vault_synth);
    } catch (e) {
      setLinkError({ install, code: String(e) });
    } finally {
      setLinking(null);
    }
  };

  return (
    <>
      <h1 className="page-title">{s.nav.cookies}</h1>
      <p className="page-lead">{t.lead(name)}</p>

      <Section>
        <Notice>{t.rule(name)}</Notice>
      </Section>

      <Section title={t.sitesTitle} note={rows.length > 0 ? t.sitesNote(name) : undefined}>
        {several ? (
          <div className="onb-actions" style={{ marginTop: 0, marginBottom: 8 }}>
            <span>{s.account.vaultOf}</span>
            <Segmented<string>
              label={s.account.vaultOf}
              value={vault_synth ?? links[0]!.synth_id}
              onChange={(id) => void refresh(id)}
              options={links.map((l) => ({ value: l.synth_id, title: linkName(l) }))}
            />
          </div>
        ) : null}
        <div className="group">
          {rows.length === 0 ? (
            <div className="empty">{t.sitesEmpty(name)}</div>
          ) : (
            rows.map((row) => (
              <div className="login-site" key={row.site}>
                <div className="login-name selectable">{row.site}</div>
                {row.password ? (
                  <div className="login-line">
                    <span className="login-what">{t.password(name)}</span>
                    <span className="login-meta">{passwordFacts(row.password, t, lang)}</span>
                    <button type="button" className="btn small" onClick={() => setRemoving(row.site)}>
                      {t.remove}
                    </button>
                  </div>
                ) : null}
                {row.sessions.map((g) => (
                  <div className="login-line" key={g.synth_id}>
                    <span className="login-what">
                      {t.session}
                      {grantSynth(state, g) ? <> <span className="tag">{grantSynth(state, g)}</span></> : null}
                    </span>
                    <span className="login-meta">{grantTime(g, s, lang)}</span>
                    <button type="button" className="btn small" onClick={() => void act(() => backend.revokeGrant("cookies", row.site, g.synth_id))}>
                      {t.revoke}
                    </button>
                  </div>
                ))}
              </div>
            ))
          )}
        </div>
        {vaultNote ? (
          <div className="login-vault-note" role="status">
            <span>{vaultNote.text}</span>
            {vaultNote.retry ? (
              <button type="button" className="btn small" disabled={asking} onClick={() => void refresh(vault_synth)}>
                {t.retry}
              </button>
            ) : null}
          </div>
        ) : null}
        {c.mode === "ask" && sessions.length > 0 ? <p className="section-note">{s.cookies.listUnused}</p> : null}
      </Section>

      <Section title={t.browsersTitle} note={t.howToShare}>
        <p className="section-lead">{t.browsersLead(name)}</p>
        {!link_ready ? (
          <div style={{ marginBottom: 8 }}>
            <Notice kind="warn">{t.linkBusy}</Notice>
          </div>
        ) : null}
        <div className="group">
          {browsers.length === 0 ? (
            <div className="empty">{t.noBrowsers}</div>
          ) : (
            browsers.map((b) => (
              <div className="row app-row" key={b.install ?? b.browser}>
                <div className="row-text">
                  <div className="row-title">
                    {b.name}
                    {several && b.state === "linked" && b.synth_name ? <> <span className="tag">{b.synth_name}</span></> : null}
                  </div>
                  <div className="row-desc">{browserLine(b, t, name)}</div>
                  {b.state === "missing" ? <div className="row-desc">{missingHint(b, t)}</div> : null}
                  {b.state === "linked" ? (
                    <div className="app-perm yes">
                      <IconCheck />
                      <span>{b.all_sites ? t.linkedAllSites : b.sites.length > 0 ? t.linkedSites(b.sites.join(", ")) : t.linkedNoSites}</span>
                    </div>
                  ) : null}
                  {b.all_sites ? <div className="row-desc">{t.allSites(b.name, name)}</div> : null}
                  {b.safari?.standalone ? <div className="row-desc">{t.safariStandalone}</div> : null}
                  {linkError && linkError.install === b.install ? (
                    <p className="error-text" role="alert">
                      {t.linkError(linkError.code, name)}
                    </p>
                  ) : null}
                </div>
                {b.state === "missing" && b.safari ? (
                  b.safari.enabled === "translocated" ? null : (
                    <button
                      type="button"
                      className={b.safari.enabled === "on" ? "btn small" : "btn small strong"}
                      onClick={() => void backend.openExternal("safari_extension")}
                    >
                      {t.safariSettings}
                    </button>
                  )
                ) : b.state === "missing" ? (
                  <button type="button" className="btn small" onClick={() => void backend.openExternal("vault_extension")}>
                    {t.getExtension}
                  </button>
                ) : b.state === "linked" ? (
                  <button type="button" className="btn small" onClick={() => setUnlinking(b)}>
                    {t.unlink}
                  </button>
                ) : b.connected ? (
                  <button
                    type="button"
                    className="btn small strong"
                    disabled={linking !== null}
                    onClick={() => {
                      // More than one synth: the owner picks whose vault.
                      if (several) {
                        setChosen(vault_synth ?? links[0]!.synth_id);
                        setChoosing(b);
                      } else void link(b.install!, links[0]?.synth_id ?? null);
                    }}
                  >
                    {linking === b.install ? t.linking : t.linkBrowser}
                  </button>
                ) : (
                  <button type="button" className="btn small" onClick={() => void act(() => backend.browserForget(b.install!))}>
                    {t.forget}
                  </button>
                )}
              </div>
            ))
          )}
        </div>
      </Section>

      <Section title={t.signinsTitle}>
        <div className="group">
          <SwitchRow strong title={s.cookies.enable(name)} checked={c.enabled} onChange={(v) => edit((d) => void (d.cookies.enabled = v))} />
        </div>
        <div style={{ marginTop: 8 }}>
          <Notice kind={c.enabled ? "warn" : "info"}>{s.cookies.warning(name)}</Notice>
        </div>
      </Section>

      <fieldset disabled={!c.enabled}>
        <Section title={s.cookies.modeTitle(name)}>
          <ChoiceList<CookiesMode>
            label={s.cookies.modeTitle(name)}
            value={c.mode}
            onChange={(v) => edit((d) => void (d.cookies.mode = v))}
            options={[
              { value: "ask", title: s.cookies.ask, desc: s.cookies.askDesc },
              { value: "ask_new", title: s.cookies.askNew, desc: s.cookies.askNewDesc(name) },
              { value: "auto", title: s.cookies.auto, desc: s.cookies.autoDesc(name) },
            ]}
          />
        </Section>
      </fieldset>

      <More label={s.common.details}>
        <div>{state.app.os === "windows" ? s.cookies.browsersWindows : s.cookies.browsers}</div>
        <div style={{ marginTop: 4 }}>{t.browserAsks}</div>
        {browsers.some((b) => b.browser === "safari") ? <div style={{ marginTop: 4 }}>{t.safariAsks(name)}</div> : null}
        {state.app.os === "macos" ? <div style={{ marginTop: 4 }}>{s.cookies.keychainNote}</div> : null}
      </More>

      {removing ? (
        <Sheet
          title={t.removeTitle(removing)}
          onCancel={() => setRemoving(null)}
          actions={
            <>
              <button type="button" className="btn" data-autofocus onClick={() => setRemoving(null)}>
                {s.common.cancel}
              </button>
              <button
                type="button"
                className="btn danger solid"
                onClick={() => {
                  const site = removing;
                  setRemoving(null);
                  void act(() => backend.vaultForget(site, vault_synth));
                }}
              >
                {t.removeConfirm}
              </button>
            </>
          }
        >
          <p style={{ marginTop: 10, lineHeight: 1.45 }}>{t.removeBody(name, removing)}</p>
        </Sheet>
      ) : null}

      {choosing ? (
        <Sheet
          title={s.account.whichSynthTitle(choosing.name)}
          onCancel={() => setChoosing(null)}
          actions={
            <>
              <button type="button" className="btn" onClick={() => setChoosing(null)}>
                {s.common.cancel}
              </button>
              <button
                type="button"
                className="btn primary"
                data-autofocus
                onClick={() => {
                  const install = choosing.install!;
                  setChoosing(null);
                  void link(install, chosen);
                }}
              >
                {t.linkBrowser}
              </button>
            </>
          }
        >
          <p style={{ marginTop: 10, marginBottom: 12, lineHeight: 1.45 }}>{s.account.whichSynthBody}</p>
          <ChoiceList<string>
            label={s.account.whichSynthTitle(choosing.name)}
            value={chosen}
            onChange={setChosen}
            options={links.map((l) => ({ value: l.synth_id, title: linkName(l), desc: s.link[l.state] }))}
          />
        </Sheet>
      ) : null}

      {unlinking ? (
        <Sheet
          title={t.unlinkTitle(unlinking.name)}
          onCancel={() => setUnlinking(null)}
          actions={
            <>
              <button type="button" className="btn" data-autofocus onClick={() => setUnlinking(null)}>
                {s.common.cancel}
              </button>
              <button
                type="button"
                className="btn danger solid"
                onClick={() => {
                  const install = unlinking.install!;
                  setUnlinking(null);
                  void act(() => backend.browserUnlink(install));
                }}
              >
                {t.unlink}
              </button>
            </>
          }
        >
          <p style={{ marginTop: 10, lineHeight: 1.45 }}>{t.unlinkBody(name)}</p>
        </Sheet>
      ) : null}
    </>
  );
}

type T = ReturnType<typeof useStore>["s"]["logins"];

function passwordFacts(p: VaultSite, t: T, lang: ReturnType<typeof useStore>["lang"]): string {
  const facts: string[] = [];
  if (p.updated > 0) facts.push(t.passwordSaved(formatDay(p.updated, lang)));
  facts.push(p.use_count > 0 && p.last_used > 0 ? t.passwordUsed(p.use_count, formatDay(p.last_used, lang)) : t.passwordUnused);
  if (p.has_totp) facts.push(t.passwordTotp);
  return facts.join(" · ");
}

function browserLine(b: BrowserRow, t: T, name: string): string {
  if (b.state === "missing") {
    if (!b.safari) return t.missing;
    // Safari's extension is already on the computer (inside the app):
    // what is missing is the owner switching it on.
    if (b.safari.enabled === "translocated") return t.safariMoved;
    return b.safari.enabled === "on" ? t.safariOn : t.safariOff;
  }
  // Safari stops an extension it finds idle, so "not connected" does not
  // mean that Safari is closed.
  if (b.state === "linked") return b.connected ? t.linked(name) : b.safari ? t.safariLinkedAway(name) : t.linkedAway(name);
  return b.connected ? t.installed(b.ext_version || "?") : b.safari ? t.safariInstalledAway : t.installedAway;
}

function missingHint(b: BrowserRow, t: T): string {
  if (!b.safari) return t.missingHint;
  if (b.safari.enabled === "translocated") return t.safariMovedHint;
  return b.safari.enabled === "on" ? t.safariOnHint : t.safariOffHint;
}
