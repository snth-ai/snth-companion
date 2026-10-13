import { useEffect, useState } from "react";
import { backend } from "../backend";
import type { HealthItem, HealthReport, UiState } from "../backend/types";
import { acceleratorFromEvent, formatClock, hostOf, shortcutKeys } from "../lib/format";
import { useStore } from "../lib/store";
import { Keys, More, Notice, Row, Section, Segmented, Sheet, SwitchRow } from "../ui/controls";
import { IconBlock, IconCheck, IconX } from "../ui/icons";

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

/** What stops this copy from updating itself, in plain words. Also
 * shown on the Status page: on macOS it means the app should be moved. */
export function UpdateBlockedNotice() {
  const { state, s } = useStore();
  const t = s.general;
  const blocked = state.update.blocked;
  if (!state.update.configured || !blocked) return null;
  return (
    <Notice
      kind="warn"
      title={t.blockedTitle}
      actions={
        blocked === "package" ? (
          <button type="button" className="btn small" onClick={() => void backend.openExternal("releases")}>
            {t.blockedDownload}
          </button>
        ) : undefined
      }
    >
      {t.blocked[blocked] ?? t.blocked.read_only}
    </Notice>
  );
}

function Updates() {
  const { state, s, lang, act } = useStore();
  const t = s.general;
  const u = state.update;
  const [busy, setBusy] = useState(false);

  const run = (work: () => Promise<UiState>) => {
    setBusy(true);
    void act(work).then(() => setBusy(false));
  };

  const percent = u.phase === "downloading" && u.total ? Math.min(100, Math.round((u.received / u.total) * 100)) : null;
  const line = !u.configured
    ? t.notConfigured
    : u.phase === "checking"
      ? t.checking
      : u.phase === "downloading"
        ? t.downloading(u.version, percent)
        : u.phase === "ready"
          ? t.ready(u.version)
          : u.phase === "available"
            ? t.available(u.version)
            : u.phase === "applying"
              ? t.applying(u.version)
              : u.phase === "failed"
                ? u.step === "check"
                  ? t.failedCheck
                  : u.step === "download"
                    ? t.failedDownload(u.version ?? "")
                    : t.failedInstall(u.version ?? "")
                : u.updated_from
                  ? t.updatedFrom(u.updated_from)
                  : u.last_checked
                    ? t.lastChecked(formatClock(u.last_checked, lang))
                    : "";
  const working = busy || u.phase === "checking" || u.phase === "downloading" || u.phase === "applying";
  // A version that can be installed with one click right now.
  const installable = !u.blocked && (u.phase === "ready" || u.phase === "available");

  return (
    <>
      <div className="row">
        <div className="row-text">
          <div className="row-title">{t.version(state.app.version)}</div>
          {line ? (
            <div className={`row-desc${u.phase === "failed" ? " error-text" : ""}`} aria-live="polite">
              {line}
              {installable && u.notes ? <div style={{ marginTop: 3 }}>{u.notes}</div> : null}
            </div>
          ) : null}
          {u.phase === "downloading" ? (
            <div className="meter" aria-hidden="true">
              <i style={{ width: `${percent ?? 4}%` }} />
            </div>
          ) : null}
        </div>
        {installable ? (
          <button type="button" className="btn primary" disabled={working} onClick={() => run(() => backend.installUpdate())}>
            {u.phase === "ready" ? t.restartNow : t.install}
          </button>
        ) : (
          <button type="button" className="btn" disabled={!u.configured || working} onClick={() => run(() => backend.checkUpdate())}>
            {t.check}
          </button>
        )}
      </div>
      <SwitchRow
        title={t.autoUpdate}
        desc={t.autoUpdateDesc}
        checked={u.auto && u.configured && !u.blocked}
        disabled={!u.configured || u.blocked !== null}
        onChange={(v) => void act(() => backend.setAutoUpdate(v))}
      />
    </>
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

/** "Check the helper": what the helper for computer control says about
 * itself, line by line, in the owner's words. The helper's own English
 * goes into "Copy details" only. */
function HelperCheck() {
  const { state, s, name } = useStore();
  const t = s.general.health;
  const [phase, setPhase] = useState<"idle" | "running" | "failed">("idle");
  const [report, setReport] = useState<HealthReport | null>(null);
  const [copied, setCopied] = useState(false);

  const run = () => {
    setPhase("running");
    backend
      .cuaHealthCheck()
      .then((next) => {
        setReport(next);
        setPhase("idle");
      })
      .catch(() => setPhase("failed"));
  };

  const note = (item: HealthItem): string => {
    const keys = [`${item.id}:${item.code}`, `${item.id}:${item.verdict}:${state.app.os}`, `${item.id}:${item.verdict}`];
    for (const key of keys) {
      const make = t.notes[key];
      if (make) return make(name, item.value);
    }
    return "";
  };

  return (
    <div className="health">
      <div className="health-head">
        <div>
          <div className="health-title">{t.title}</div>
          <div>{t.lead}</div>
        </div>
        <button type="button" className="btn small" disabled={phase === "running"} onClick={run}>
          {phase === "running" ? t.running : report ? t.again : t.run}
        </button>
      </div>
      {phase === "failed" ? (
        <div className="error-text" role="alert">
          {t.failed}
        </div>
      ) : null}
      {report ? (
        <div aria-live="polite">
          <div className={`health-overall ${report.overall}`}>{t.overall[report.overall] ?? report.overall}</div>
          <ul className="health-list">
            {report.items.map((item) => {
              const text = note(item);
              return (
                <li key={item.id} className={`health-item ${item.verdict}`}>
                  <span className="health-mark" title={t.mark[item.verdict]} aria-label={t.mark[item.verdict]}>
                    {item.verdict === "ok" ? <IconCheck /> : item.verdict === "fail" ? <IconBlock /> : <span aria-hidden="true">-</span>}
                  </span>
                  <span className="health-text">
                    <span className="health-name">{item.id === "pointer" ? s.computer.cursor(name) : (t.items[item.id] ?? t.other(item.id))}</span>
                    {text ? <span className="health-note">{text}</span> : null}
                  </span>
                </li>
              );
            })}
          </ul>
          <div className="onb-actions">
            <button
              type="button"
              className="btn small"
              onClick={() =>
                void copyText(report.details).then((ok) => {
                  setCopied(ok);
                  window.setTimeout(() => setCopied(false), 2500);
                })
              }
            >
              {copied ? s.general.copied : t.copy}
            </button>
            <span>{t.copyNote}</span>
          </div>
        </div>
      ) : null}
    </div>
  );
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
      <HelperCheck />
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
        {state.update.blocked && state.update.configured ? (
          <div style={{ marginTop: 8 }}>
            <UpdateBlockedNotice />
          </div>
        ) : null}
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
