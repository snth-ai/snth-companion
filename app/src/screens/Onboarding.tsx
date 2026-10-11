// First run, and again after unlinking: welcome, code, confirmation, then
// the choices that decide what the synth may do. Nothing is linked before
// the Connect click on the confirmation step, and nothing is allowed
// before a preset is applied.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { backend } from "../backend";
import type { Preset, SuggestedRoot } from "../backend/types";
import { hostOf, shortPath, summarize } from "../lib/format";
import { useStore } from "../lib/store";
import { Notice, Switch } from "../ui/controls";
import { IconCheck, IconFolder, IconPlus, IconX } from "../ui/icons";
import { HelperStatus } from "./HelperStatus";

export type Step = "welcome" | "code" | "confirm" | "preset" | "folders" | "computer" | "done";
const ORDER: Step[] = ["welcome", "code", "confirm", "preset", "folders", "computer", "done"];
const BEFORE_LINK: Step[] = ["welcome", "code", "confirm"];

type Choice = Exclude<Preset, "custom">;

function Frame(props: { step: Step; center?: boolean; children: ReactNode; foot: ReactNode }) {
  const { s } = useStore();
  const index = ORDER.indexOf(props.step);
  return (
    <div className="onb">
      <div className={`onb-body${props.center ? " center" : ""}`}>
        <div className={`onb-col${props.center ? " center" : ""}`}>{props.children}</div>
      </div>
      <div className="onb-foot">
        <span className="onb-steps">{index > 0 ? s.onboarding.stepOf(index, ORDER.length - 1) : ""}</span>
        {props.foot}
      </div>
    </div>
  );
}

export function Onboarding(props: { initialStep?: Step }) {
  const { state, s, name, act, edit, linkCode, takeLinkCode } = useStore();
  const t = s.onboarding;
  const paired = state.core.paired;
  const settings = state.core.settings;

  const [step, setStep] = useState<Step>(() => {
    if (props.initialStep) return props.initialStep;
    if (paired) return "preset";
    return state.pending ? "confirm" : "welcome";
  });
  const [code, setCode] = useState("");
  const [hub, setHub] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [fromLink, setFromLink] = useState(false);
  const [preset, setPreset] = useState<Choice>(settings.preset === "custom" ? "careful" : settings.preset);
  const [suggested, setSuggested] = useState<SuggestedRoot[]>([]);
  const [checked, setChecked] = useState<Record<string, boolean>>({});
  const [autostart, setAutostart] = useState(true);
  const codeRef = useRef<HTMLInputElement>(null);

  // The link state decides which half of the flow can be shown.
  const shown: Step = paired ? (BEFORE_LINK.includes(step) ? "preset" : step) : BEFORE_LINK.includes(step) ? step : "welcome";

  const claim = async (value: string, viaLink: boolean) => {
    setBusy(true);
    setError(null);
    try {
      await backend.pairBegin(value, hub.trim() || undefined);
      const next = await backend.getState();
      setFromLink(viaLink);
      await act(async () => next);
      setStep("confirm");
    } catch (e) {
      const key = String(e).split(":")[0]!.trim();
      setError(t.errors[key] ?? t.errors.hub_error!);
      setStep("code");
    } finally {
      setBusy(false);
    }
  };

  // A `snth://pair` link: ask the hub who the code belongs to and show the
  // confirmation. The link alone never connects anything.
  useEffect(() => {
    if (!linkCode || paired) return;
    takeLinkCode();
    setCode(linkCode);
    void claim(linkCode, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [linkCode, paired]);

  useEffect(() => {
    if (shown === "code") codeRef.current?.focus();
    if (shown === "folders") {
      void backend.suggestedRoots().then((roots) => {
        setSuggested(roots);
        setChecked((c) => {
          const next = { ...c };
          for (const r of roots) if (!(r.path in next)) next[r.path] = true;
          return next;
        });
      });
    }
  }, [shown]);

  const back = (to: Step) => (
    <button type="button" className="btn quiet" onClick={() => setStep(to)}>
      {s.common.back}
    </button>
  );

  if (shown === "welcome") {
    return (
      <Frame
        step="welcome"
        center
        foot={
          <button type="button" className="btn primary large" onClick={() => setStep("code")}>
            {t.start}
          </button>
        }
      >
        <div className="onb-welcome">
          <img className="onb-icon" src="/app-icon.png" alt="" />
          <h1>{t.welcomeTitle}</h1>
          {t.welcomeBody.map((line) => (
            <p key={line}>{line}</p>
          ))}
        </div>
      </Frame>
    );
  }

  if (shown === "code") {
    return (
      <Frame
        step="code"
        foot={
          <>
            {back("welcome")}
            <button type="button" className="btn primary" disabled={busy || code.length !== 6} onClick={() => void claim(code, false)}>
              {busy ? t.checking : s.common.continue}
            </button>
          </>
        }
      >
        <h1>{t.codeTitle}</h1>
        <p className="onb-lead">{t.codeBody}</p>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (code.length === 6 && !busy) void claim(code, false);
          }}
        >
          <input
            ref={codeRef}
            className="code-input"
            inputMode="numeric"
            autoComplete="one-time-code"
            aria-label={t.codeLabel}
            aria-invalid={error !== null}
            placeholder="000000"
            value={code}
            disabled={busy}
            onChange={(e) => {
              setCode(e.target.value.replace(/\D/g, "").slice(0, 6));
              setError(null);
            }}
          />
        </form>
        {error ? (
          <p className="error-text" role="alert" style={{ fontSize: 13 }}>
            {error}
          </p>
        ) : null}
        <details className="more" style={{ marginTop: 22 }}>
          <summary>{t.advanced}</summary>
          <div className="more-body">
            <label style={{ display: "block", marginBottom: 5 }} htmlFor="hub">
              {t.hubLabel}
            </label>
            <input
              id="hub"
              className="input"
              style={{ width: "100%" }}
              value={hub}
              placeholder={state.app.hub_url}
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(e) => setHub(e.target.value)}
            />
            <div style={{ marginTop: 5 }}>{t.hubHint}</div>
          </div>
        </details>
      </Frame>
    );
  }

  if (shown === "confirm") {
    const pending = state.pending;
    const cancel = () => {
      void backend.pairCancel().then(() => act(() => backend.getState()));
      setCode("");
      setStep("code");
    };
    if (!pending) {
      // The claimed code is gone (the app restarted, or it was cancelled).
      return (
        <Frame step="confirm" foot={back("code")}>
          <h1>{t.codeTitle}</h1>
          <p className="onb-lead">{t.errors.code_expired}</p>
        </Frame>
      );
    }
    return (
      <Frame
        step="confirm"
        foot={
          <>
            <button type="button" className="btn" disabled={busy} onClick={cancel}>
              {s.common.cancel}
            </button>
            <button
              type="button"
              className="btn primary"
              disabled={busy}
              onClick={() => {
                setBusy(true);
                void act(() => backend.pairConfirm()).then((ok) => {
                  setBusy(false);
                  if (ok) setStep("preset");
                });
              }}
            >
              {busy ? t.connecting : t.connect}
            </button>
          </>
        }
      >
        <h1>{t.confirmTitle(pending.synth_id)}</h1>
        <div className="group facts">
          <div className="row">
            <span className="row-label">{t.confirmSynth}</span>
            <span className="row-text selectable" style={{ fontWeight: 600 }}>
              {pending.synth_id}
            </span>
          </div>
          <div className="row">
            <span className="row-label">{t.confirmAddress}</span>
            <span className="row-text selectable">{hostOf(pending.synth_url)}</span>
          </div>
          <div className="row">
            <span className="row-label">{t.confirmComputer}</span>
            <span className="row-text">{state.app.device_name}</span>
          </div>
        </div>
        <p className="onb-lead" style={{ fontSize: 13 }}>
          {t.confirmBody}
        </p>
        {fromLink ? (
          <div style={{ marginTop: 14 }}>
            <Notice kind="warn">{t.confirmFromLink}</Notice>
          </div>
        ) : null}
      </Frame>
    );
  }

  if (shown === "preset") {
    const options: { value: Choice; title: string; lines: string[] }[] = [
      { value: "careful", title: t.careful, lines: t.carefulLines(name) },
      { value: "balanced", title: t.balanced, lines: t.balancedLines(name) },
      { value: "full", title: t.full, lines: t.fullLines(name) },
    ];
    return (
      <Frame
        step="preset"
        foot={
          <button
            type="button"
            className="btn primary"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void act(() => backend.applyPreset(preset)).then((ok) => {
                setBusy(false);
                if (ok) setStep("folders");
              });
            }}
          >
            {s.common.continue}
          </button>
        }
      >
        <h1>{t.presetTitle(name)}</h1>
        <p className="onb-lead">{t.presetBody}</p>
        <div className="group preset" role="radiogroup" aria-label={t.presetTitle(name)} style={{ marginTop: 16 }}>
          {options.map((o) => (
            <button key={o.value} type="button" role="radio" className="choice" aria-checked={preset === o.value} onClick={() => setPreset(o.value)}>
              <span className="radio" />
              <span className="row-text">
                <span className="preset-title">{o.title}</span>
                {o.lines.map((line) => (
                  <span className="preset-line" style={{ display: "block" }} key={line}>
                    {line}
                  </span>
                ))}
              </span>
            </button>
          ))}
        </div>
      </Frame>
    );
  }

  if (shown === "folders") {
    const roots = settings.files.roots;
    const suggestedPaths = suggested.map((r) => r.path);
    const extra = roots.filter((r) => !suggestedPaths.includes(r.path));
    const anyFolder = suggested.some((r) => checked[r.path]) || extra.length > 0;

    const apply = async () => {
      setBusy(true);
      for (const r of suggested) {
        const present = roots.some((x) => x.path === r.path);
        if (checked[r.path] && !present) await act(() => backend.addSuggestedRoot(r.path, true));
        if (!checked[r.path] && present) await act(() => backend.removeRoot(r.path));
      }
      setBusy(false);
      setStep("computer");
    };

    return (
      <Frame
        step="folders"
        foot={
          <>
            {back("preset")}
            <button type="button" className="btn primary" disabled={busy} onClick={() => void apply()}>
              {s.common.continue}
            </button>
          </>
        }
      >
        <h1>{t.foldersTitle(name)}</h1>
        <p className="onb-lead">{t.foldersBody(name)}</p>
        <div className="group" style={{ marginTop: 16 }}>
          {suggested.map((r) => (
            <div className="item" key={r.path} style={{ paddingLeft: 12 }}>
              <button
                type="button"
                role="checkbox"
                className="check"
                aria-checked={checked[r.path] ?? false}
                aria-label={r.kind === "desktop" ? t.desktop : t.downloads}
                onClick={() => setChecked((c) => ({ ...c, [r.path]: !c[r.path] }))}
              >
                {checked[r.path] ? <IconCheck /> : null}
              </button>
              <span className="item-text" title={r.path}>
                {r.kind === "desktop" ? t.desktop : t.downloads}
              </span>
              <span className="item-sub">{shortPath(r.path)}</span>
            </div>
          ))}
          {extra.map((r) => (
            <div className="item" key={r.path} style={{ paddingLeft: 13 }}>
              <IconFolder />
              <span className="item-text" title={r.path}>
                {shortPath(r.path)}
              </span>
              <button
                type="button"
                className="btn quiet icon-only"
                title={s.common.remove}
                aria-label={`${s.common.remove}: ${r.path}`}
                onClick={() => void act(() => backend.removeRoot(r.path))}
              >
                <IconX />
              </button>
            </div>
          ))}
          <div className="add-row">
            <button type="button" className="btn" onClick={() => void act(() => backend.pickFolder(true))}>
              <IconPlus />
              {s.files.addFolder}
            </button>
          </div>
        </div>
        {!anyFolder ? <p className="section-note">{t.foldersNone(name)}</p> : null}
        {settings.computer.enabled ? <p className="section-note">{s.files.reach(name)}</p> : null}
      </Frame>
    );
  }

  if (shown === "computer") {
    const mac = state.app.os === "macos";
    return (
      <Frame
        step="computer"
        foot={
          <>
            {back("folders")}
            {settings.computer.enabled ? (
              <button
                type="button"
                className="btn"
                title={t.computerSkipNote}
                onClick={() => {
                  edit((d) => void (d.computer.enabled = false));
                  setStep("done");
                }}
              >
                {t.computerSkip}
              </button>
            ) : null}
            <button type="button" className="btn primary" onClick={() => setStep("done")}>
              {s.common.continue}
            </button>
          </>
        }
      >
        <h1>{t.computerTitle}</h1>
        <p className="onb-lead">{mac ? t.computerBody(name) : t.computerBodyOther(name)}</p>
        <div style={{ marginTop: 16 }}>
          <HelperStatus live notReadyNote={settings.computer.enabled ? t.computerLater : t.computerSkipNote} />
        </div>
      </Frame>
    );
  }

  // done
  const where = state.app.os === "macos" ? s.general.whereMac : s.general.whereOther;
  const on: { title: string; cap: "net" | "cookies" | "computer" | "files"; enabled: boolean }[] = [
    { title: s.nav.internet, cap: "net", enabled: settings.net.enabled },
    { title: s.nav.cookies, cap: "cookies", enabled: settings.cookies.enabled },
    { title: s.nav.computer, cap: "computer", enabled: settings.computer.enabled },
    { title: s.nav.files, cap: "files", enabled: settings.files.enabled },
  ];
  const enabled = on.filter((o) => o.enabled);
  return (
    <Frame
      step="done"
      foot={
        <>
          {back("computer")}
          <button
            type="button"
            className="btn primary"
            disabled={busy}
            onClick={() => {
              setBusy(true);
              void act(() => backend.finishOnboarding(autostart)).then(() => setBusy(false));
            }}
          >
            {t.finish}
          </button>
        </>
      }
    >
      <h1>{t.doneTitle}</h1>
      <p className="onb-lead">{t.doneBody(name, where)}</p>
      <h2 className="section-title" style={{ marginTop: 18 }}>
        {enabled.length > 0 ? t.doneOn : t.doneNothing}
      </h2>
      {enabled.length > 0 ? (
        <div className="group">
          {enabled.map((o) => (
            <div className="row" key={o.cap} style={{ minHeight: 38 }}>
              <span className="row-text">{o.title}</span>
              <span className="row-value">{summarize(o.cap, settings, s)}</span>
            </div>
          ))}
        </div>
      ) : null}
      <div className="group" style={{ marginTop: 14 }}>
        <div className="row">
          <span className="row-text">{t.doneAutostart}</span>
          <Switch checked={autostart} onChange={setAutostart} label={t.doneAutostart} />
        </div>
      </div>
    </Frame>
  );
}
