// What the synths see of this computer (connect.md "Observe"): per synth,
// a switch for each source it takes right now (calls, the app in front,
// the place), off until the owner turns it on after reading what it
// sends. Under the switches, each source that is on: what it sends now,
// the apps never shown (the activity), the named Wi-Fi networks (the
// place).
//
// The page asks the shell for a fresh view every two seconds while it is
// open (the live "now sending" line); everything else comes with the state.

import { useEffect, useMemo, useState } from "react";
import { backend } from "../backend";
import type { AppMark, ObserveDescriptor, ObserveLink, ObserveState } from "../backend/types";
import { formatClock } from "../lib/format";
import { useStore } from "../lib/store";
import { Notice, Row, Section, Sheet, Switch } from "../ui/controls";
import { IconPlus, IconX } from "../ui/icons";

const SOURCES = ["device.call", "device.activity", "place.current"] as const;
type Source = (typeof SOURCES)[number];

const LIVE_EVERY_MS = 2000;

/** The page's data: the state's copy, kept fresh while the page is open. */
function useObserve(): ObserveState | null {
  const { state } = useStore();
  const [live, setLive] = useState<ObserveState | null>(null);
  useEffect(() => {
    let alive = true;
    const read = () =>
      void backend
        .observeView()
        .then((v) => alive && setLive(v))
        .catch(() => undefined);
    read();
    const id = window.setInterval(read, LIVE_EVERY_MS);
    return () => {
      alive = false;
      window.clearInterval(id);
    };
  }, []);
  // A change the owner just made arrives with the state first.
  useEffect(() => setLive(null), [state.observe]);
  return live ?? state.observe ?? null;
}

function words(w: { en: string; ru: string } | undefined, lang: string): string {
  if (!w) return "";
  return (lang === "ru" ? w.ru || w.en : w.en || w.ru).trim();
}

function supported(o: ObserveState, r: Source): boolean {
  return r === "device.call" ? o.support.call : r === "device.activity" ? o.support.activity : o.support.place;
}

/** The consent sheet: what this source sends, before it is switched on. */
function ConsentSheet(props: { link: ObserveLink; resource: Source; descriptor?: ObserveDescriptor; onCancel: () => void; onConfirm: () => void }) {
  const { s, lang, state } = useStore();
  const t = s.observe;
  const who = props.link.synth_name || props.link.synth_id;
  const consent = words(props.descriptor?.consent, lang);
  const os = state.app.os;
  return (
    <Sheet
      title={t.consentTitle(t.source[props.resource] ?? props.resource, who)}
      onCancel={props.onCancel}
      actions={
        <>
          <button type="button" className="btn" onClick={props.onCancel}>
            {s.common.cancel}
          </button>
          <button type="button" className="btn primary" data-autofocus onClick={props.onConfirm}>
            {t.turnOn}
          </button>
        </>
      }
    >
      <p>{consent || (props.resource === "place.current" ? t.consentPlace(who) : t.consentFallback(who))}</p>
      {props.resource === "place.current" && consent ? <p className="observe-consent-more">{t.consentPlace(who)}</p> : null}
      {props.resource === "place.current" && os === "macos" ? <p className="row-desc observe-consent-more">{t.consentPlaceMac}</p> : null}
      {props.resource === "place.current" && os === "windows" ? <p className="row-desc observe-consent-more">{t.consentPlaceWindows}</p> : null}
      <p className="row-desc observe-consent-more">{t.pauseNote}</p>
    </Sheet>
  );
}

/** One synth's switches. */
function LinkSources(props: { link: ObserveLink; o: ObserveState; many: boolean; onAsk: (link: ObserveLink, r: Source) => void }) {
  const { s, lang, act } = useStore();
  const t = s.observe;
  const { link, o } = props;
  const who = link.synth_name || link.synth_id;
  const offered = SOURCES.filter((r) => link.offered.includes(r));

  let body;
  if (!link.feature) {
    body = <Notice kind="info">{t.outdated(who)}</Notice>;
  } else if (offered.length === 0) {
    body = (
      <Notice
        kind="info"
        actions={
          link.linked ? (
            <button type="button" className="btn small" onClick={() => void act(() => backend.observeRefresh(link.synth_id))}>
              {t.askAgain}
            </button>
          ) : undefined
        }
      >
        {t.noneOffered(who)}
      </Notice>
    );
  } else {
    body = (
      <div className="group">
        {offered.map((r) => {
          const d = link.descriptors.find((x) => x.resource === r);
          const on = link.on.includes(r);
          const here = supported(o, r);
          const title = t.source[r] ?? r;
          return (
            <Row key={r} title={title} desc={here ? t.sourceDesc[r] || words(d?.description, lang) : t.notSupportedHere}>
              <Switch
                label={title}
                checked={on}
                disabled={!here && !on}
                onChange={(next) => {
                  if (next) props.onAsk(link, r);
                  else void act(() => backend.observeSet(link.synth_id, r, false));
                }}
              />
            </Row>
          );
        })}
      </div>
    );
  }

  return (
    <Section title={props.many ? who : undefined}>
      {body}
      {link.feature && !link.linked ? (
        <div style={{ marginTop: 8 }}>
          <Notice kind="info">{t.offline(who)}</Notice>
        </div>
      ) : null}
    </Section>
  );
}

/** "Now sending": the value of one source as the synth gets it. */
function Sending(props: { o: ObserveState; resource: Source }) {
  const { s } = useStore();
  const t = s.observe;
  const v = props.o.values[props.resource]?.value as Record<string, unknown> | undefined;
  let text: string = t.sendingNothing;
  if (v && props.resource === "device.activity") {
    if (v.category === "private") text = t.sendingPrivate;
    else text = [v.app, v.title].filter((x) => typeof x === "string" && x).join(", ") + ` (${t.categories[String(v.category)] ?? String(v.category)})`;
  }
  return (
    <div className="observe-sending" aria-live="polite">
      <span className="observe-sending-label">{t.sendingNow}:</span> <span className="selectable">{text}</span>
    </div>
  );
}

function CallDetails(props: { o: ObserveState }) {
  const { s, lang } = useStore();
  const t = s.observe;
  const v = props.o.values["device.call"]?.value as { in_call?: boolean; app?: string; since?: string } | undefined;
  let line: string = t.callNone;
  if (v?.in_call) {
    const app = t.callApps[v.app ?? "other"] ?? t.callApps.other!;
    const since = v.since ? Date.parse(v.since) : NaN;
    line = Number.isFinite(since) ? t.callNow(app, formatClock(since / 1000, lang)) : t.callNowNoSince(app);
  }
  return (
    <Section title={t.callTitle} note={t.callHow}>
      <div className="group">
        <Row title={line} />
      </div>
    </Section>
  );
}

/** The exclusion picker: the apps lately in front and the ones running. */
function AppPicker(props: { o: ObserveState; onClose: () => void }) {
  const { s, act } = useStore();
  const t = s.observe;
  const [running, setRunning] = useState<AppMark[] | null>(null);
  useEffect(() => {
    let alive = true;
    void backend
      .observeRunningApps()
      .then((list) => alive && setRunning(list))
      .catch(() => alive && setRunning([]));
    return () => {
      alive = false;
    };
  }, []);
  const excluded = props.o.settings.excluded ?? [];
  const isOut = (a: AppMark) => excluded.some((m) => m.id.toLowerCase() === a.id.toLowerCase());
  const recent = props.o.recent.filter((a) => !isOut(a));
  const others = (running ?? []).filter((a) => !isOut(a) && !recent.some((r) => r.id.toLowerCase() === a.id.toLowerCase()));
  const row = (a: AppMark) => (
    <div className="item" key={a.id}>
      <span className="item-text" title={a.id}>
        {a.name || a.id}
      </span>
      <button type="button" className="btn small" onClick={() => void act(() => backend.observeExclude(a.id, a.name, true))}>
        {t.pickerAdd}
      </button>
    </div>
  );
  return (
    <Sheet
      title={t.pickerTitle}
      onCancel={props.onClose}
      actions={
        <button type="button" className="btn primary" data-autofocus onClick={props.onClose}>
          {t.done}
        </button>
      }
    >
      <div className="picker-list">
        {recent.length > 0 ? (
          <>
            <div className="observe-picker-head">{t.pickerRecent}</div>
            {recent.map(row)}
          </>
        ) : null}
        <div className="observe-picker-head">{t.pickerRunning}</div>
        {running === null ? (
          <div className="empty" role="status">
            {t.pickerLoading}
          </div>
        ) : others.length === 0 ? (
          <div className="empty">{t.pickerNone}</div>
        ) : (
          others.map(row)
        )}
      </div>
    </Sheet>
  );
}

function ActivityDetails(props: { o: ObserveState }) {
  const { s, state, act } = useStore();
  const t = s.observe;
  const [picking, setPicking] = useState(false);
  const own = props.o.settings.excluded ?? [];
  const defaults = state.app.os === "macos" ? t.defaultsMac : t.defaultsOther;
  return (
    <Section title={t.activityTitle}>
      <div className="group">
        <div className="row">
          <div className="row-text">
            <Sending o={props.o} resource="device.activity" />
          </div>
        </div>
      </div>
      {props.o.titles_allowed === false ? (
        <div style={{ marginTop: 8 }}>
          <Notice
            kind="info"
            actions={
              <button type="button" className="btn small" onClick={() => void backend.openExternal("privacy_accessibility")}>
                {t.openSettings}
              </button>
            }
          >
            {t.titlesNeedAccess}
          </Notice>
        </div>
      ) : null}

      <h3 className="observe-sub">{t.neverTitle}</h3>
      <p className="section-note" style={{ marginTop: 0 }}>
        {t.neverNote}
      </p>
      <div className="group">
        <div className="row">
          <div className="row-text">
            <div className="row-desc">{defaults.join(", ")}</div>
          </div>
          <span className="tag">{t.always}</span>
        </div>
        {own.map((m) => (
          <div className="item" key={`o-${m.id || m.name}`}>
            <span className="item-text" title={m.id}>
              {m.name || m.id}
            </span>
            <button
              type="button"
              className="btn quiet icon-only"
              aria-label={`${s.common.remove}: ${m.name || m.id}`}
              title={s.common.remove}
              onClick={() => void act(() => backend.observeExclude(m.id, m.name, false))}
            >
              <IconX />
            </button>
          </div>
        ))}
      </div>
      <div className="onb-actions">
        <button type="button" className="btn small" onClick={() => setPicking(true)}>
          <IconPlus />
          {t.addApp}
        </button>
      </div>
      {picking ? <AppPicker o={props.o} onClose={() => setPicking(false)} /> : null}
    </Section>
  );
}

/** A field to name (or rename) a network. */
function NameField(props: { initial: string; onSave: (name: string) => Promise<boolean>; onCancel?: () => void }) {
  const { s } = useStore();
  const t = s.observe;
  const [value, setValue] = useState(props.initial);
  const [busy, setBusy] = useState(false);
  return (
    <form
      className="add-row"
      onSubmit={(e) => {
        e.preventDefault();
        if (!value.trim()) return;
        setBusy(true);
        void props.onSave(value.trim()).finally(() => setBusy(false));
      }}
    >
      <input
        className="input"
        value={value}
        maxLength={64}
        placeholder={t.namePlaceholder}
        aria-label={t.nameIt}
        autoFocus={!!props.onCancel}
        onChange={(e) => setValue(e.target.value)}
      />
      <button type="submit" className="btn" disabled={!value.trim() || busy}>
        {t.save}
      </button>
      {props.onCancel ? (
        <button type="button" className="btn quiet" onClick={props.onCancel}>
          {s.common.cancel}
        </button>
      ) : null}
    </form>
  );
}

function PlaceDetails(props: { o: ObserveState }) {
  const { s, state, act } = useStore();
  const t = s.observe;
  const { o } = props;
  const [renaming, setRenaming] = useState<string | null>(null);
  const places = o.settings.places ?? [];
  const win = state.app.os === "windows";
  const save = async (ssid: string, name: string) => {
    const ok = await act(() => backend.observePlace(ssid, name));
    if (ok) setRenaming(null);
    return ok;
  };

  let current;
  const w = o.wifi;
  if (w.state === "network") {
    const named = places.find((p) => p.ssid === w.ssid);
    current = (
      <div className="group">
        <Row title={t.wifiNow(w.ssid)} desc={named ? t.placeAt(named.name) : t.placeUnnamed} />
        {!named ? (
          <div className="row">
            <div className="row-text">
              <NameField key={w.ssid} initial="" onSave={(name) => save(w.ssid, name)} />
            </div>
          </div>
        ) : null}
      </div>
    );
  } else if (w.state === "not_asked") {
    current = (
      <Notice
        kind="info"
        actions={
          <button type="button" className="btn small" onClick={() => void act(() => backend.observeLocation())}>
            {t.allow}
          </button>
        }
      >
        {t.notAsked}
      </Notice>
    );
  } else if (w.state === "denied") {
    current = (
      <Notice
        kind="warn"
        actions={
          <button type="button" className="btn small" onClick={() => void backend.openExternal("privacy_location")}>
            {t.openSettings}
          </button>
        }
      >
        {win ? t.deniedWindows : t.deniedMac}
      </Notice>
    );
  } else {
    const line = w.state === "not_connected" ? t.noWifi : w.state === "unsupported" ? t.wifiUnsupported : t.reading;
    current = (
      <div className="group">
        <Row title={line} />
      </div>
    );
  }

  return (
    <Section title={t.placeTitle}>
      {current}
      <h3 className="observe-sub">{t.placesTitle}</h3>
      <div className="group">
        {places.length === 0 ? <div className="empty">{t.noPlaces}</div> : null}
        {places.map((p) =>
          renaming === p.ssid ? (
            <div className="row" key={p.ssid}>
              <div className="row-text">
                <NameField initial={p.name} onSave={(name) => save(p.ssid, name)} onCancel={() => setRenaming(null)} />
              </div>
            </div>
          ) : (
            <div className="item" key={p.ssid}>
              <span className="item-text">
                {p.name} <span className="item-sub selectable">{p.ssid}</span>
              </span>
              <button type="button" className="btn small quiet" onClick={() => setRenaming(p.ssid)}>
                {t.rename}
              </button>
              <button type="button" className="btn small quiet" onClick={() => void act(() => backend.observePlace(p.ssid, ""))}>
                {t.forget}
              </button>
            </div>
          ),
        )}
      </div>
      <p className="section-note">{t.placesNote}</p>
    </Section>
  );
}

export function Observe() {
  const { state, s, act, name } = useStore();
  const t = s.observe;
  const o = useObserve();
  const [asking, setAsking] = useState<{ link: ObserveLink; resource: Source } | null>(null);
  const links = o?.links ?? [];
  const many = links.length > 1;
  const onSomewhere = useMemo(() => new Set(links.flatMap((l) => l.on)), [links]);
  const anySupported = o ? SOURCES.some((r) => supported(o, r)) : false;

  return (
    <>
      <h1 className="page-title">{s.nav.observe(many ? null : name)}</h1>
      <p className="page-lead">{many ? t.leadMany : t.lead(name)}</p>

      {state.core.paused ? (
        <Section>
          <Notice kind="warn">{t.paused}</Notice>
        </Section>
      ) : null}
      {o && !anySupported ? (
        <Section>
          <Notice kind="info">{t.unsupported}</Notice>
        </Section>
      ) : null}

      {o ? links.map((l) => <LinkSources key={l.synth_id} link={l} o={o} many={many} onAsk={(link, resource) => setAsking({ link, resource })} />) : null}

      {o && onSomewhere.has("device.call") && o.support.call ? <CallDetails o={o} /> : null}
      {o && onSomewhere.has("device.activity") && o.support.activity ? <ActivityDetails o={o} /> : null}
      {o && onSomewhere.has("place.current") && o.support.place ? <PlaceDetails o={o} /> : null}

      {asking ? (
        <ConsentSheet
          link={asking.link}
          resource={asking.resource}
          descriptor={asking.link.descriptors.find((d) => d.resource === asking.resource)}
          onCancel={() => setAsking(null)}
          onConfirm={() => {
            const { link, resource } = asking;
            setAsking(null);
            void act(() => backend.observeSet(link.synth_id, resource, true));
          }}
        />
      ) : null}
    </>
  );
}
