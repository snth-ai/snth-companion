// The test iPhone (macOS only): one switch, which phone over Wi-Fi (or the
// cable), where the screen and the keyboard board stand, a look at the
// phone's screen, the one-time setup of the phone, and the synths the
// owner allowed for good.
//
// The synth sees the phone's screen over the USB cable (the capture
// helper inside the app) or over Wi-Fi (Xcode's devicectl, for the one
// phone the owner picks here; nothing is ever picked by itself), and taps
// and types through the SNTH Phone Bridge board (a Bluetooth keyboard and
// mouse for the phone). Each synth asks the owner first; a permission for
// one synth is not one for another.

import { useCallback, useEffect, useState } from "react";
import { backend } from "../backend";
import type { PhoneDevice, PhonePreview, PhoneView } from "../backend/types";
import { useStore } from "../lib/store";
import { ChoiceList, Notice, Section, SwitchRow } from "../ui/controls";
import { IconCheck, IconWarning } from "../ui/icons";
import { Remembered } from "./Remembered";

/** The phone's state, re-read when the settings change and every few
 * seconds while the phone is on (the cable and the board come and go). */
function usePhone(on: boolean) {
  const { state } = useStore();
  const [view, setView] = useState<PhoneView | null>(null);
  const [checking, setChecking] = useState(false);
  const rev = state.core.settings_rev;

  const check = useCallback(async () => {
    setChecking(true);
    try {
      setView(await backend.phoneCheck());
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
    if (!on) return;
    const id = window.setInterval(() => void check(), 3000);
    return () => window.clearInterval(id);
  }, [check, on]);

  return { view, checking, check };
}

/** The phones this Mac sees over Wi-Fi: asked when the page opens and on
 * "Find phones". A list only; the owner picks. */
function usePhones() {
  const [phones, setPhones] = useState<PhoneDevice[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [finding, setFinding] = useState(false);
  const find = useCallback(async () => {
    setFinding(true);
    try {
      setPhones(await backend.phoneDevices());
      setError(null);
    } catch (e) {
      setError(typeof e === "string" ? e : "");
    } finally {
      setFinding(false);
    }
  }, []);
  useEffect(() => {
    void find();
  }, [find]);
  return { phones, error, finding, find };
}

/** Cable, or one phone over Wi-Fi. The chosen phone stays listed when it
 * is not seen right now. */
function WifiPicker() {
  const { state, s, edit } = useStore();
  const t = s.phone;
  const chosen = state.core.settings.phone_control.device_udid ?? "";
  const { phones, error, finding, find } = usePhones();
  const list = phones ?? [];
  const options = [
    { value: "", title: t.wifiCable, desc: t.wifiCableDesc },
    ...list.map((p) => ({ value: p.udid, title: p.name || p.model, desc: `${t.wifiDevice(p.model, t.wifiState(p.state))} \u00b7 ${p.udid}` })),
  ];
  if (chosen && phones !== null && !list.some((p) => p.udid === chosen)) {
    options.push({ value: chosen, title: t.wifiMissing(chosen), desc: t.wifiMissingDesc });
  }
  return (
    <Section title={t.wifiTitle} note={t.wifiNote}>
      <ChoiceList<string>
        label={t.wifiTitle}
        value={chosen}
        options={options}
        onChange={(v) => edit((d) => void (d.phone_control.device_udid = v))}
      />
      {finding && phones === null ? <p className="row-desc">{t.wifiFinding}</p> : null}
      {phones !== null && list.length === 0 && error === null ? <p className="row-desc">{t.wifiNone}</p> : null}
      {error !== null ? <p className="error-text">{error || t.wifiError}</p> : null}
      <div className="onb-actions">
        <button type="button" className="btn small" disabled={finding} onClick={() => void find()}>
          {t.wifiFind}
        </button>
      </div>
    </Section>
  );
}

function StateLine(props: { ok: boolean; text: string; why?: string }) {
  return (
    <div className={`app-perm ${props.ok ? "yes" : "no"}`}>
      {props.ok ? <IconCheck /> : <IconWarning />}
      <span>
        {props.text}
        {props.why ? <span className="phone-why"> {props.why}</span> : null}
      </span>
    </div>
  );
}

export function Phone() {
  const { state, s, name, edit } = useStore();
  const t = s.phone;
  const on = state.core.settings.phone_control.enabled;
  const { view, checking, check } = usePhone(on);
  const [shot, setShot] = useState<PhonePreview | null>(null);
  const [shotError, setShotError] = useState(false);
  const [looking, setLooking] = useState(false);

  // A picture of a phone that is switched off is stale.
  useEffect(() => {
    if (!on) setShot(null);
  }, [on]);

  const look = async () => {
    setLooking(true);
    setShotError(false);
    try {
      setShot(await backend.phonePreview());
    } catch {
      setShot(null);
      setShotError(true);
    } finally {
      setLooking(false);
    }
  };

  const screen = view?.screen;
  const hands = view?.hands;

  return (
    <>
      <h1 className="page-title">{s.nav.phone}</h1>
      <p className="page-lead">{t.lead(name)}</p>

      <Section>
        <div className="group">
          <SwitchRow strong title={t.enable} checked={on} onChange={(v) => edit((d) => void (d.phone_control.enabled = v))} />
        </div>
        <div style={{ marginTop: 8 }}>
          <Notice kind={on ? "warn" : "info"}>{t.warning}</Notice>
        </div>
      </Section>

      <WifiPicker />

      {on ? (
        <Section title={t.nowTitle}>
          <div className="group">
            <div className="row app-row">
              <div className="row-text">
                <div className="row-title">{t.screenTitle}</div>
                {screen ? (
                  screen.ok ? (
                    <StateLine
                      ok
                      text={(screen.source === "network" ? t.screenOkNet : t.screenOk)(screen.device || t.thePhone, screen.width, screen.height)}
                    />
                  ) : (
                    <StateLine ok={false} text={t.screenNo} why={screen.error} />
                  )
                ) : (
                  <div className="row-desc">{t.checking}</div>
                )}
              </div>
            </div>
            <div className="row app-row">
              <div className="row-text">
                <div className="row-title">{t.handsTitle}</div>
                {hands ? (
                  !hands.ok ? (
                    <StateLine ok={false} text={t.boardNo} why={hands.error} />
                  ) : hands.ble_connected ? (
                    <StateLine ok text={t.boardOk(hands.port, hands.firmware)} />
                  ) : (
                    <StateLine ok={false} text={t.bluetoothNo(hands.port)} />
                  )
                ) : (
                  <div className="row-desc">{t.checking}</div>
                )}
              </div>
            </div>
            {view?.holder ? (
              <div className="row">
                <div className="row-text">
                  <div className="row-desc">{t.holder(view.holder)}</div>
                </div>
              </div>
            ) : null}
          </div>
          <div className="onb-actions">
            <button type="button" className="btn small" disabled={checking} onClick={() => void check()}>
              {t.checkAgain}
            </button>
            <button type="button" className="btn small" disabled={looking || !screen?.ok} onClick={() => void look()}>
              {t.showScreen}
            </button>
          </div>
          {shotError ? <p className="error-text">{t.noShot}</p> : null}
          {shot ? (
            <figure className="phone-shot">
              <img src={shot.data_url} width={shot.width} height={shot.height} alt={t.shotAlt} />
              <figcaption>{t.shotNote(shot.native_width, shot.native_height)}</figcaption>
            </figure>
          ) : null}
        </Section>
      ) : null}

      <Section title={t.setupTitle} note={t.setupNote}>
        <ol className="phone-steps">
          {t.setup.map((step) => (
            <li key={step}>{step}</li>
          ))}
        </ol>
      </Section>

      <Remembered capability="phone_control" title={t.rememberedTitle} />
    </>
  );
}
