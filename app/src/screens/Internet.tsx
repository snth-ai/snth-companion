import { backend } from "../backend";
import type { NetMode } from "../backend/types";
import { formatBytes, formatUntil, monthName } from "../lib/format";
import { grantSynth, useStore } from "../lib/store";
import { ChoiceList, ListEditor, Notice, Row, Section, Switch, SwitchRow } from "../ui/controls";

const GB = 1024 * 1024 * 1024;
const LIMITS_GB = [1, 5, 10, 25, 50, 100];

/** What the person typed -> a bare site name, or null. The shell cleans
 * it again; this is only to answer at once when it is not a site. */
function looksLikeSite(raw: string): boolean {
  const d = raw
    .trim()
    .toLowerCase()
    .replace(/^[a-z]+:\/\//, "")
    .split(/[/?#:]/)[0]!;
  return /^[^\s.]+(\.[^\s.]+)+$/.test(d);
}

export function Internet() {
  const { state, s, lang, name, edit, act } = useStore();
  const net = state.core.settings.net;
  const t = s.internet;

  // The list shows the sites the person wrote down, plus the answers they
  // gave in a request window (those carry how long the answer lasts).
  const grants = state.grants.filter((g) => g.capability === "net");
  const items = [
    ...net.domains.map((d) => ({ key: `d:${d}`, label: d })),
    ...grants
      .filter((g) => !net.domains.includes(g.scope))
      .map((g) => {
        const when = g.until === 0 ? t.tagAlways : g.until < 0 ? t.tagSession : t.tagUntil(formatUntil(g.until, lang));
        const who = grantSynth(state, g);
        // The synth id goes into the key: answers are kept per synth.
        return { key: `g:${g.synth_id}\n${g.scope}`, label: g.scope, tag: who ? `${who}, ${when}` : when };
      }),
  ];

  const revokeAll = (site: string) => {
    for (const g of grants.filter((x) => x.scope === site)) void act(() => backend.revokeGrant("net", site, g.synth_id));
  };

  const removeSite = (key: string) => {
    if (key.startsWith("g:")) {
      const [synthId, site] = key.slice(2).split("\n") as [string, string];
      void act(() => backend.revokeGrant("net", site, synthId));
    } else {
      const site = key.slice(2);
      edit((d) => {
        d.net.domains = d.net.domains.filter((x) => x !== site);
      });
      revokeAll(site);
    }
  };

  const cap = net.monthly_cap_bytes;
  const used = state.core.egress_bytes;
  const limitOptions = [...new Set([...LIMITS_GB, ...(cap > 0 ? [Math.round(cap / GB)] : [])])].sort((a, b) => a - b);
  const full = cap > 0 && used >= cap;

  return (
    <>
      <h1 className="page-title">{s.nav.internet}</h1>
      <p className="page-lead">{t.lead(name)}</p>

      <Section>
        <div className="group">
          <SwitchRow strong title={t.enable(name)} checked={net.enabled} onChange={(v) => edit((d) => void (d.net.enabled = v))} />
        </div>
      </Section>

      <fieldset disabled={!net.enabled}>
        <Section title={t.modeTitle}>
          <ChoiceList<NetMode>
            label={t.modeTitle}
            value={net.mode}
            onChange={(v) => edit((d) => void (d.net.mode = v))}
            options={[
              { value: "all", title: t.all, desc: t.allDesc },
              { value: "ask", title: t.ask, desc: t.askDesc(name) },
              { value: "list", title: t.list, desc: t.listDesc },
            ]}
          />
        </Section>

        {net.mode === "all" ? null : (
          <Section title={net.mode === "list" ? t.sitesList : t.sitesAsk}>
            <ListEditor
              items={items}
              placeholder={t.sitePlaceholder}
              addLabel={t.addSite}
              removeLabel={s.common.remove}
              emptyText={t.sitesEmpty}
              onAdd={(value) => {
                if (!looksLikeSite(value)) return t.badSite;
                edit((d) => void d.net.domains.push(value));
                return null;
              }}
              onRemove={removeSite}
            />
          </Section>
        )}

        <Section>
          <div className="group">
            <SwitchRow
              title={t.localTitle}
              desc={t.localDesc}
              checked={net.allow_local_network}
              onChange={(v) => edit((d) => void (d.net.allow_local_network = v))}
            />
          </div>
          {net.allow_local_network ? (
            <div style={{ marginTop: 8 }}>
              <Notice kind="warn">{t.localWarning(name)}</Notice>
            </div>
          ) : null}
        </Section>

        <Section>
          <div className="group">
            <Row title={t.limitTitle} desc={t.limitDesc(name)}>
              <select
                className="select"
                aria-label={t.limitTitle}
                value={cap > 0 ? Math.round(cap / GB) : 0}
                onChange={(e) => edit((d) => void (d.net.monthly_cap_bytes = Number(e.target.value) * GB))}
              >
                <option value={0}>{t.limitNone}</option>
                {limitOptions.map((gb) => (
                  <option key={gb} value={gb}>
                    {t.limitGb(gb)}
                  </option>
                ))}
              </select>
            </Row>
            <Row
              title={t.meteredTitle}
              desc={state.app.metered_detection ? t.meteredDesc : `${t.meteredDesc} ${t.meteredUnavailable}`}
            >
              <Switch
                label={t.meteredTitle}
                checked={net.skip_on_metered && state.app.metered_detection}
                disabled={!state.app.metered_detection}
                onChange={(v) => edit((d) => void (d.net.skip_on_metered = v))}
              />
            </Row>
          </div>
          {state.core.metered && net.skip_on_metered ? (
            <div style={{ marginTop: 8 }}>
              <Notice kind="warn">{t.meteredNow(name)}</Notice>
            </div>
          ) : null}
        </Section>
      </fieldset>

      <Section title={`${t.usageTitle}: ${monthName(state.core.egress_month, lang)}`}>
        <div className="group">
          <div className="row" style={{ display: "block" }}>
            <div>{cap > 0 ? t.usedOf(formatBytes(used, s, lang), formatBytes(cap, s, lang)) : t.used(formatBytes(used, s, lang))}</div>
            {cap > 0 ? (
              <div className={`meter${full ? " full" : ""}`} aria-hidden="true">
                <i style={{ width: `${Math.min(100, (used / cap) * 100)}%` }} />
              </div>
            ) : null}
          </div>
        </div>
        {full ? <p className="error-text">{t.capReached(name)}</p> : null}
      </Section>
    </>
  );
}
