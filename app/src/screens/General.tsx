import { useEffect, useState } from "react";
import { backend } from "../backend";
import type { UpdateInfo } from "../backend/types";
import { acceleratorFromEvent, hostOf, shortcutKeys } from "../lib/format";
import { useStore } from "../lib/store";
import { Keys, More, Row, Section, Segmented, Sheet, SwitchRow } from "../ui/controls";
import { IconX } from "../ui/icons";

type LangChoice = "system" | "en" | "ru";

function StopShortcut() {
  const { state, s, act } = useStore();
  const t = s.general;
  const os = state.app.os;
  const [recording, setRecording] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const current = state.prefs.stop_shortcut;

  useEffect(() => {
    if (!recording) return;
    const onKey = (e: KeyboardEvent) => {
      e.preventDefault();
      e.stopPropagation();
      if (e.key === "Escape" && !e.metaKey && !e.ctrlKey && !e.altKey) {
        setRecording(false);
        return;
      }
      const accelerator = acceleratorFromEvent(e, os);
      if (accelerator === null) return;
      if (accelerator === "invalid") {
        setProblem(t.shortcutNeedsModifier);
        return;
      }
      setRecording(false);
      void backend
        .setStopShortcut(accelerator)
        .then((next) => {
          setProblem(null);
          return act(async () => next);
        })
        .catch(() => setProblem(t.shortcutTaken));
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [recording, os, act, t.shortcutNeedsModifier, t.shortcutTaken]);

  const inactive = current !== "" && !state.prefs.stop_shortcut_active;

  return (
    <div className="row">
      <div className="row-text">
        <div className="row-title">{t.shortcut}</div>
        <div className="row-desc">{t.shortcutDesc}</div>
        {problem || inactive ? (
          <div className="error-text" role="alert">
            {problem ?? t.shortcutInactive}
          </div>
        ) : null}
      </div>
      {recording ? (
        <span className="muted" aria-live="polite">
          {t.shortcutRecording}
        </span>
      ) : current ? (
        <Keys keys={shortcutKeys(current, os)} />
      ) : (
        <span className="muted">{t.shortcutNone}</span>
      )}
      {recording ? (
        <button type="button" className="btn small" onClick={() => setRecording(false)}>
          {s.common.cancel}
        </button>
      ) : (
        <>
          <button
            type="button"
            className="btn small"
            onClick={() => {
              setProblem(null);
              setRecording(true);
            }}
          >
            {t.shortcutChange}
          </button>
          {current ? (
            <button
              type="button"
              className="btn quiet icon-only"
              title={t.shortcutOff}
              aria-label={t.shortcutOff}
              onClick={() => {
                setProblem(null);
                void act(() => backend.setStopShortcut(""));
              }}
            >
              <IconX />
            </button>
          ) : null}
        </>
      )}
    </div>
  );
}

function Updates() {
  const { state, s } = useStore();
  const t = s.general;
  const [phase, setPhase] = useState<"idle" | "checking" | "installing" | "failed">("idle");
  const [info, setInfo] = useState<UpdateInfo | null>(null);

  const check = async () => {
    setPhase("checking");
    try {
      setInfo(await backend.checkUpdate());
      setPhase("idle");
    } catch {
      setInfo(null);
      setPhase("failed");
    }
  };

  const install = async () => {
    setPhase("installing");
    try {
      await backend.installUpdate();
      setInfo(null);
      setPhase("idle");
    } catch {
      setPhase("failed");
    }
  };

  const line = !state.app.updater_ready
    ? t.notConfigured
    : phase === "checking"
      ? t.checking
      : phase === "installing"
        ? t.installing
        : phase === "failed"
          ? t.updateFailed
          : info?.status === "up_to_date"
            ? t.upToDate
            : info?.status === "available"
              ? t.available(info.version ?? "")
              : info?.status === "not_configured"
                ? t.notConfigured
                : "";

  return (
    <div className="row">
      <div className="row-text">
        <div className="row-title">{t.version(state.app.version)}</div>
        {line ? (
          <div className={`row-desc${phase === "failed" ? " error-text" : ""}`} aria-live="polite">
            {line}
            {info?.status === "available" && info.notes && phase === "idle" ? <div style={{ marginTop: 3 }}>{info.notes}</div> : null}
          </div>
        ) : null}
      </div>
      {info?.status === "available" && phase !== "checking" ? (
        <button type="button" className="btn primary" disabled={phase === "installing"} onClick={() => void install()}>
          {t.install}
        </button>
      ) : (
        <button type="button" className="btn" disabled={!state.app.updater_ready || phase === "checking"} onClick={() => void check()}>
          {t.check}
        </button>
      )}
    </div>
  );
}

async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const area = document.createElement("textarea");
    area.value = text;
    area.style.position = "fixed";
    area.style.opacity = "0";
    document.body.appendChild(area);
    area.select();
    const ok = document.execCommand("copy");
    area.remove();
    return ok;
  }
}

function Diagnostics() {
  const { state, s } = useStore();
  const t = s.general;
  const core = state.core;
  const [copied, setCopied] = useState(false);

  const linkText = core.paused
    ? s.link.paused
    : state.needs_repair
      ? s.link.repair
      : s.link[core.paired ? core.link_state : "unpaired"];

  const facts: [string, string][] = [
    [t.diagApp, `${state.app.version}${state.app.debug_build ? " (debug)" : ""}, ${state.app.os} ${state.app.arch}`],
    [
      t.diagHelper,
      state.helper.state === "ready"
        ? `${state.helper.version} (${core.cua_status})`
        : state.helper.state === "failed"
          ? `${t.diagHelperNone} (${state.helper.code})`
          : t.diagHelperBusy,
    ],
    [t.diagSynth, core.synth_version ?? "-"],
    [t.diagLink, state.link_detail ? `${linkText} (${state.link_detail})` : linkText],
    [t.diagAddress, hostOf(core.synth_url) || "-"],
    [t.diagComputer, state.app.device_name],
    [t.diagKey, state.app.secret_store],
    [t.diagHub, hostOf(state.app.hub_url)],
  ];

  return (
    <More label={t.diagnostics}>
      {facts.map(([label, value]) => (
        <div key={label}>
          {label}: {value}
        </div>
      ))}
      <div className="onb-actions">
        <button
          type="button"
          className="btn small"
          onClick={() =>
            void backend
              .diagnostics()
              .then(copyText)
              .then((ok) => {
                setCopied(ok);
                window.setTimeout(() => setCopied(false), 2500);
              })
              .catch(() => undefined)
          }
        >
          {copied ? t.copied : t.copy}
        </button>
        <span>{t.copyNote}</span>
      </div>
    </More>
  );
}

export function General() {
  const { state, s, lang, name, act } = useStore();
  const t = s.general;
  const [confirming, setConfirming] = useState(false);
  const [unlinking, setUnlinking] = useState(false);
  const [autostartFailed, setAutostartFailed] = useState(false);
  const choice: LangChoice = state.prefs.language === "en" || state.prefs.language === "ru" ? state.prefs.language : "system";
  const where = state.app.os === "macos" ? t.whereMac : t.whereOther;

  const setLanguage = (next: LangChoice) => {
    const language = next === "system" ? null : next;
    void act(() => backend.setLanguage(language, language ?? lang));
  };

  const unlink = async () => {
    setUnlinking(true);
    await act(() => backend.unpair());
    setUnlinking(false);
    setConfirming(false);
  };

  return (
    <>
      <h1 className="page-title">{s.nav.general}</h1>

      <Section>
        <div className="group">
          <Row title={t.language}>
            <Segmented<LangChoice>
              label={t.language}
              value={choice}
              onChange={setLanguage}
              options={[
                { value: "system", title: t.languageSystem },
                { value: "en", title: "English" },
                { value: "ru", title: "Русский" },
              ]}
            />
          </Row>
          <SwitchRow
            title={t.autostart}
            desc={
              <>
                {t.autostartDesc(where)}
                {autostartFailed ? <span className="error-text" style={{ display: "block" }}>{t.autostartFailed}</span> : null}
              </>
            }
            checked={state.prefs.autostart}
            onChange={(v) => void act(() => backend.setAutostart(v)).then((ok) => setAutostartFailed(!ok))}
          />
          <StopShortcut />
        </div>
      </Section>

      <Section title={t.updates}>
        <div className="group">
          <Updates />
        </div>
      </Section>

      <Section>
        <div className="group">
          <Row title={t.unlink} desc={t.unlinkDesc(name)}>
            <button type="button" className="btn danger" onClick={() => setConfirming(true)}>
              {t.unlinkButton}
            </button>
          </Row>
        </div>
      </Section>

      <Diagnostics />

      {confirming ? (
        <Sheet
          title={t.unlinkTitle(name)}
          onCancel={() => (unlinking ? undefined : setConfirming(false))}
          actions={
            <>
              <button type="button" className="btn" data-autofocus disabled={unlinking} onClick={() => setConfirming(false)}>
                {s.common.cancel}
              </button>
              <button type="button" className="btn danger solid" disabled={unlinking} onClick={() => void unlink()}>
                {unlinking ? t.unlinking : t.unlinkConfirm}
              </button>
            </>
          }
        >
          <p style={{ marginTop: 10 }}>{t.unlinkErases}</p>
          <ul className="sheet-list">
            {t.unlinkItems(name).map((item) => (
              <li key={item}>
                <IconX />
                <span>{item}</span>
              </li>
            ))}
          </ul>
          <p className="muted" style={{ marginTop: 12, lineHeight: 1.45 }}>
            {t.unlinkKept} {t.unlinkAgain}
          </p>
        </Sheet>
      ) : null}
    </>
  );
}
