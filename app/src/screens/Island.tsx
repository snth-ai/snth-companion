// The Island screen: the synth's face at the top of the screen and the
// hold keys. Off by default (a test feature). The face is the synth's
// own: the editor saves it to the synth (face.set), and the synth may
// change it too, or design one itself when asked. Speech recognition:
// through the synth (the default) or on this computer, with the model's
// download, status and removal. Wake by name: the switch, the names it
// answers to, an extra phrase, what it does now, its own small model.

import { useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import type { FaceConfig, HotkeyKind, SttEngine, SttMode, UiState } from "../backend/types";
import { FaceEditor } from "../island/faceApi";
import { sttEngines } from "../island/sttView";
import { acceleratorFromEvent, formatBytes, shortcutKeys } from "../lib/format";
import { linkName, useStore } from "../lib/store";
import { ChoiceList, Keys, Notice, Row, Section, SwitchRow } from "../ui/controls";
import { IconX } from "../ui/icons";

const KINDS: HotkeyKind[] = ["voice", "screen_voice", "text"];

function HotkeyRow(props: { kind: HotkeyKind; name: string }) {
  const { state, s, set } = useStore();
  const t = s.island;
  const os = state.app.os;
  const isl = state.island;
  const current = isl.settings.hotkeys[props.kind];
  const [recording, setRecording] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const explain = (e: unknown): string => {
    const code = String(e);
    if (code.startsWith("conflict:")) return t.keyConflict[code.slice("conflict:".length)] ?? t.keyTaken;
    if (code.startsWith("bad_shortcut")) return t.keyBad;
    return t.keyTaken;
  };
  const apply = (work: () => Promise<UiState>) =>
    void work()
      .then((next) => {
        setProblem(null);
        set(next);
      })
      .catch((e) => setProblem(explain(e)));

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
        setProblem(t.keyNeedsModifier);
        return;
      }
      setRecording(false);
      apply(() => backend.islandHotkeySet(props.kind, accelerator));
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  });

  const system = isl.key_problems[props.kind];
  const shown = problem ?? (system === "taken" ? t.keyTaken : system === "bad" ? t.keyBad : null);
  const isDefault = current === isl.defaults[props.kind];

  return (
    <div className="row">
      <div className="row-text">
        <div className="row-title">{t.key[props.kind]}</div>
        <div className="row-desc">{t.keyDesc[props.kind](props.name)}</div>
        {shown ? (
          <div className="error-text" role="alert">
            {shown}
          </div>
        ) : null}
      </div>
      {recording ? (
        <span className="muted" aria-live="polite">
          {t.keyRecording}
        </span>
      ) : current ? (
        <Keys keys={shortcutKeys(current, os)} />
      ) : (
        <span className="muted">{t.keyNone}</span>
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
            {t.keyChange}
          </button>
          {isDefault ? null : (
            <button type="button" className="btn small quiet" onClick={() => apply(() => backend.islandHotkeyReset(props.kind))}>
              {t.keyReset}
            </button>
          )}
          {current ? (
            <button
              type="button"
              className="btn quiet icon-only"
              title={t.keyOff}
              aria-label={t.keyOff}
              onClick={() => apply(() => backend.islandHotkeySet(props.kind, ""))}
            >
              <IconX />
            </button>
          ) : null}
        </>
      )}
    </div>
  );
}

/** Speech recognition: through the synth or on this computer, the
 * engine on this computer, and the model each engine needs. */
function SttSection(props: { name: string; takesText: boolean }) {
  const { state, s, lang, act } = useStore();
  const t = s.island;
  const isl = state.island;
  const stt = isl.stt;
  const mode = isl.settings.stt;
  // The engine that runs (Parakeet on Windows even when the settings say
  // whisper, the default), and the engines of this build.
  const { chosen, engines, pick } = sttEngines(stt, isl.settings.stt_engine);
  const [problem, setProblem] = useState<string | null>(null);
  const run = (work: () => Promise<UiState>) => {
    setProblem(null);
    void act(() => work().catch((e) => {
      // The status says why (no_space); anything else is said here.
      if (String(e) !== "no_space") setProblem(t.sttFailed.io!("", ""));
      return null;
    }));
  };

  if (!stt.available) {
    return (
      <Section title={t.stt}>
        <Notice kind="info">{t.sttUnavailable}</Notice>
      </Section>
    );
  }

  const slot = (e: SttEngine) => stt.engines?.[e] ?? { model: e === chosen ? stt.model : ({ state: "missing" } as const), size: stt.size };

  /** One engine's model: its status and what can be done with it. */
  const modelRow = (e: SttEngine) => {
    const { model, size: bytes } = slot(e);
    const size = formatBytes(bytes, s, lang);
    let status: string;
    switch (model.state) {
      case "downloading": {
        const pct = Math.min(100, Math.floor((model.done * 100) / Math.max(1, model.total)));
        status = t.sttDownloading(pct, formatBytes(model.done, s, lang), formatBytes(model.total, s, lang));
        break;
      }
      case "verifying":
        status = t.sttVerifying;
        break;
      case "ready":
        status = t.sttReady(size);
        break;
      case "failed": {
        const [need, free] = model.detail.split(" ").map((n) => formatBytes(Number(n) || 0, s, lang));
        status = (t.sttFailed[model.code] ?? t.sttFailed.io!)(need ?? "", free ?? "");
        break;
      }
      default:
        status = mode === "device" && e === chosen ? t.sttMissingOn(size) : t.sttMissing(size);
    }
    return (
      <div className="group" key={e}>
        <Row title={t.sttModelOf[e]} desc={<span className={model.state === "failed" ? "error-text" : undefined}>{status}</span>}>
          {model.state === "downloading" ? (
            <button type="button" className="btn small" onClick={() => run(() => backend.islandSttCancel(e))}>
              {t.sttCancel}
            </button>
          ) : null}
          {model.state === "missing" ? (
            <button type="button" className="btn small" onClick={() => run(() => backend.islandSttDownload(e))}>
              {t.sttDownload}
            </button>
          ) : null}
          {model.state === "failed" ? (
            <button type="button" className="btn small" onClick={() => run(() => backend.islandSttDownload(e))}>
              {t.sttRetry}
            </button>
          ) : null}
          {model.state === "ready" ? (
            <button type="button" className="btn small quiet" onClick={() => run(() => backend.islandSttDelete(e))}>
              {t.sttDelete}
            </button>
          ) : null}
        </Row>
        {model.state === "downloading" ? (
          <div className="stt-progress" aria-hidden="true">
            <i style={{ width: `${Math.min(100, (model.done * 100) / Math.max(1, model.total))}%` }} />
          </div>
        ) : null}
      </div>
    );
  };

  return (
    <Section title={t.stt} note={t.sttNote}>
      <ChoiceList<SttMode>
        label={t.stt}
        value={mode}
        options={[
          { value: "synth", title: t.sttSynth, desc: t.sttSynthDesc },
          { value: "device", title: t.sttDevice, desc: t.sttDeviceDesc },
        ]}
        onChange={(next) => run(() => backend.islandSttSet(next))}
      />
      {mode === "device" && !props.takesText ? <Notice kind="warn">{t.sttSynthOld(props.name)}</Notice> : null}
      {mode === "device" && pick ? (
        <ChoiceList<SttEngine>
          label={t.sttEngine}
          value={chosen}
          options={[
            { value: "whisper", title: t.sttWhisper, desc: t.sttWhisperDesc },
            { value: "parakeet", title: t.sttParakeet, desc: t.sttParakeetDesc },
          ]}
          onChange={(next) => run(() => backend.islandSttEngineSet(next))}
        />
      ) : null}
      {mode === "device" && !pick ? (
        <div className="group">
          <Row title={t.sttEngineOnly[chosen]} desc={t.sttEngineOnlyDesc[chosen]} />
        </div>
      ) : null}
      {mode === "device" ? (
        <div className="group">
          <SwitchRow
            title={t.sttTone}
            desc={t.sttToneDesc}
            checked={isl.settings.stt_tone}
            onChange={(next) => run(() => backend.islandSttToneSet(next))}
          />
        </div>
      ) : null}
      {engines.filter((e) => (mode === "device" && e === chosen) || slot(e).model.state !== "missing").map(modelRow)}
      {problem ? (
        <div className="error-text" role="alert">
          {problem}
        </div>
      ) : null}
    </Section>
  );
}

/** Wake by name (Apple silicon and Windows x64 with AVX2; hidden
 * elsewhere). */
function WakeSection(props: { name: string }) {
  const { state, s, lang, act } = useStore();
  const t = s.island;
  const win = state.app.os === "windows";
  const isl = state.island;
  const wake = isl.wake;
  const on = isl.settings.wake;
  const saved = isl.settings.wake_phrase;
  const [phrase, setPhrase] = useState(saved);
  useEffect(() => setPhrase(saved), [saved]);
  const model = wake.model;
  const size = formatBytes(wake.size, s, lang);
  const [problem, setProblem] = useState<string | null>(null);
  const run = (work: () => Promise<UiState>) => {
    setProblem(null);
    void act(() =>
      work().catch((e) => {
        if (String(e) !== "no_space") setProblem(t.sttFailed.io!("", ""));
        return null;
      }),
    );
  };
  // The name as the owner says it: the Cyrillic spelling in Russian.
  const said = (lang === "ru" ? wake.names.find((n) => /[а-яё]/i.test(n)) : undefined) ?? wake.names[0] ?? props.name;
  const savePhrase = () => {
    if (phrase.trim() !== saved) run(() => backend.islandWakePhrase(phrase));
  };

  let modelText: string;
  switch (model.state) {
    case "downloading": {
      const pct = Math.min(100, Math.floor((model.done * 100) / Math.max(1, model.total)));
      modelText = t.wakeDownloading(pct, formatBytes(model.done, s, lang), formatBytes(model.total, s, lang));
      break;
    }
    case "verifying":
      modelText = t.sttVerifying;
      break;
    case "ready":
      modelText = t.sttReady(size);
      break;
    case "failed": {
      const [need, free] = model.detail.split(" ").map((n) => formatBytes(Number(n) || 0, s, lang));
      modelText = (t.sttFailed[model.code] ?? t.sttFailed.io!)(need ?? "", free ?? "");
      break;
    }
    default:
      modelText = t.wakeMissing(size);
  }

  let now: string;
  if (wake.state === "paused") now = t.wakePaused[wake.reason] ?? "";
  else if (wake.state === "waiting") {
    // Windows asks nothing up front: a refusal is its privacy settings.
    const w = win && wake.need === "mic" ? t.wakeWaitingMicWin : t.wakeWaiting[wake.need];
    now = typeof w === "function" ? w(props.name) : (w ?? "");
  } else now = t.wakeStatus[wake.state] ?? "";
  if (wake.wakes > 0 && (wake.state === "listening" || wake.state === "paused")) now = `${now} ${t.wakeCount(wake.wakes, wake.dropped)}`;

  return (
    <Section title={t.wake} note={win ? t.wakeMicLineWin : t.wakeMicLine}>
      <div className="group">
        <SwitchRow title={t.wakeSwitch} desc={t.wakeDesc(said)} checked={on} onChange={(next) => run(() => backend.islandWakeSet(next))} />
        {on ? (
          <>
            <Row title={t.wakeNamesTitle} desc={wake.names.length ? wake.names.join(" · ") : t.wakeNoName} />
            <Row title={t.wakePhrase} desc={t.wakePhraseDesc}>
              <input
                className="input wake-phrase"
                value={phrase}
                maxLength={40}
                placeholder={t.wakePhrasePlaceholder}
                aria-label={t.wakePhrase}
                onChange={(e) => setPhrase(e.target.value)}
                onBlur={savePhrase}
                onKeyDown={(e) => {
                  if (e.key === "Enter") e.currentTarget.blur();
                }}
              />
            </Row>
            <Row title={t.wakeNow} desc={<span className={wake.state === "paused" ? "warn-text" : undefined}>{now}</span>} />
          </>
        ) : null}
        {on || model.state !== "missing" ? (
          <>
            <Row title={t.wakeModel} desc={<span className={model.state === "failed" ? "error-text" : undefined}>{modelText}</span>}>
              {model.state === "downloading" ? (
                <button type="button" className="btn small" onClick={() => run(() => backend.islandWakeDownloadCancel())}>
                  {t.sttCancel}
                </button>
              ) : null}
              {model.state === "missing" ? (
                <button type="button" className="btn small" onClick={() => run(() => backend.islandWakeDownload())}>
                  {t.sttDownload}
                </button>
              ) : null}
              {model.state === "failed" ? (
                <button type="button" className="btn small" onClick={() => run(() => backend.islandWakeDownload())}>
                  {t.sttRetry}
                </button>
              ) : null}
              {model.state === "ready" ? (
                <button type="button" className="btn small quiet" onClick={() => run(() => backend.islandWakeDelete())}>
                  {t.sttDelete}
                </button>
              ) : null}
            </Row>
            {model.state === "downloading" ? (
              <div className="stt-progress" aria-hidden="true">
                <i style={{ width: `${Math.min(100, (model.done * 100) / Math.max(1, model.total))}%` }} />
              </div>
            ) : null}
          </>
        ) : null}
      </div>
      {problem ? (
        <div className="error-text" role="alert">
          {problem}
        </div>
      ) : null}
    </Section>
  );
}

type SaveState = { kind: "idle" } | { kind: "saving" } | { kind: "saved" } | { kind: "error"; text: string };

function FaceSection(props: { synthId: string; name: string; supported: boolean; online: boolean }) {
  const { state, s, lang } = useStore();
  const t = s.island;
  const saved = state.island.faces[props.synthId] ?? null;
  const [draft, setDraft] = useState<FaceConfig | null>(saved);
  const [save, setSave] = useState<SaveState>({ kind: "idle" });
  const [asked, setAsked] = useState<"idle" | "sent" | "failed">("idle");
  const timer = useRef<number | undefined>(undefined);
  const editing = useRef(false);

  // The synth's face changed by itself (or was read again): show it,
  // unless the owner is in the middle of a change.
  useEffect(() => {
    if (!editing.current) setDraft(saved);
  }, [saved]);

  // Read the face once when the screen opens.
  useEffect(() => {
    if (props.supported && props.online) void backend.faceRefresh(props.synthId).catch(() => undefined);
  }, [props.synthId, props.supported, props.online]);

  const onChange = (face: FaceConfig) => {
    editing.current = true;
    setDraft(face);
    setSave({ kind: "saving" });
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      void backend
        .faceSet(props.synthId, face, false)
        .then((r) => {
          editing.current = false;
          setSave(r.result === "saved" ? { kind: "saved" } : { kind: "error", text: t.faceInvalid(props.name, r.reason) });
        })
        .catch((e) => {
          editing.current = false;
          const code = String(e);
          setSave({ kind: "error", text: code === "offline" ? t.faceOffline(props.name) : code === "outdated" ? t.faceOutdated(props.name) : t.faceFailed });
        });
    }, 600);
  };
  useEffect(() => () => window.clearTimeout(timer.current), []);

  const ask = () => {
    setAsked("idle");
    void backend
      .islandAskDesign(props.synthId)
      .then(() => setAsked("sent"))
      .catch(() => setAsked("failed"));
  };

  return (
    <Section title={t.face} note={t.faceDesc(props.name)}>
      {!props.supported ? (
        <Notice kind="warn">{t.outdated(props.name)}</Notice>
      ) : (
        <>
          <div className="group island-face">
            <FaceEditor value={draft} synthId={props.synthId} onChange={onChange} lang={lang} />
            <div className="island-face-foot" aria-live="polite">
              {save.kind === "saving" ? <span className="muted">{t.faceSaving}</span> : null}
              {save.kind === "saved" ? <span className="muted">{t.faceSaved}</span> : null}
              {save.kind === "error" ? <span className="error-text">{save.text}</span> : null}
            </div>
          </div>
          <div className="island-ask">
            <button type="button" className="btn" disabled={!props.online} onClick={ask}>
              {t.askDesign(props.name)}
            </button>
            {asked === "sent" ? <span className="muted">{t.askDesignSent(props.name)}</span> : null}
            {asked === "failed" ? <span className="error-text">{t.askDesignFailed(props.name)}</span> : null}
          </div>
        </>
      )}
    </Section>
  );
}

export function Island() {
  const { state, s, act } = useStore();
  const t = s.island;
  const isl = state.island;
  const links = state.core.links;
  const mainId = isl.main_synth;
  const main = links.find((l) => l.synth_id === mainId) ?? null;
  const name = main ? linkName(main) : s.common.yourSynth;
  const online = main?.state === "linked";
  const supported = mainId ? isl.supported[mainId] !== false : false;

  return (
    <>
      <h1 className="page-title">
        {t.title} <span className="tag">{t.badge}</span>
      </h1>
      <p className="page-lead">{t.intro(name)}</p>

      {!isl.available ? (
        <Notice kind="info">{t.unavailable}</Notice>
      ) : (
        <>
          <Section>
            <div className="group">
              <SwitchRow
                title={t.enable}
                desc={t.enableDesc}
                strong
                checked={isl.settings.enabled}
                onChange={(next) => void act(() => backend.islandSet(next, null))}
              />
            </div>
          </Section>

          {isl.settings.enabled ? (
            <>
              {links.length > 1 ? (
                <Section title={t.mainSynth} note={t.mainSynthDesc}>
                  <div className="group" role="radiogroup" aria-label={t.mainSynth}>
                    {links.map((l) => (
                      <button
                        key={l.synth_id}
                        type="button"
                        role="radio"
                        className="choice"
                        aria-checked={l.synth_id === mainId}
                        onClick={() => void act(() => backend.islandSet(null, l.synth_id))}
                      >
                        <span className="radio" />
                        <span className="row-text">
                          <span className="row-title">{linkName(l)}</span>
                          {isl.supported[l.synth_id] === false ? (
                            <span className="row-desc" style={{ display: "block" }}>
                              {t.outdated(linkName(l))}
                            </span>
                          ) : null}
                        </span>
                      </button>
                    ))}
                  </div>
                </Section>
              ) : null}

              {main && !supported && links.length < 2 ? <Notice kind="warn">{t.outdated(name)}</Notice> : null}

              <Section title={t.keys} note={t.keysNote}>
                {!isl.keys_active ? <Notice kind="info">{t.keysInactive(name)}</Notice> : null}
                <div className="group">
                  {KINDS.map((k) => (
                    <HotkeyRow key={k} kind={k} name={name} />
                  ))}
                </div>
              </Section>

              <Section>
                <div className="group">
                  <Row
                    title={t.mic}
                    desc={isl.mic === "granted" ? t.micGranted : isl.mic === "denied" ? t.micDenied : isl.mic === "not_asked" ? t.micNotAsked : t.micUnknown}
                  >
                    {isl.mic === "denied" ? (
                      <button type="button" className="btn small" onClick={() => void backend.islandMic()}>
                        {t.micSettings}
                      </button>
                    ) : isl.mic === "not_asked" ? (
                      <button type="button" className="btn small" onClick={() => void backend.islandMic()}>
                        {t.micAllow}
                      </button>
                    ) : null}
                  </Row>
                </div>
              </Section>

              <SttSection name={name} takesText={mainId ? isl.device_stt[mainId] === true : false} />

              {isl.wake?.available ? <WakeSection name={name} /> : null}

              {mainId ? <FaceSection key={mainId} synthId={mainId} name={name} supported={supported} online={online} /> : null}
            </>
          ) : null}
        </>
      )}
    </>
  );
}
