// Review page for the face (browser only, mock mode). Open
// http://127.0.0.1:1420/src/island/face/demo.html while `npm run dev` runs.
//
// URL parameters:
//   view    stage (default: a fake notch + open island, then the grid),
//           grid (every look x every state), presets (every look in one
//           state), states (one look in every state)
//   size    face height in px for grid / presets / states (default 96)
//   preset  the look for stage / states (default mia)
//   state   the state for presets (default idle)
//   still   1 = settled poses, no animation (screenshots)
//   gaze    x,y fixed gaze for every face, e.g. 0.4,0.6 (screenshots)
//   emote   play this idle emote (with still=1: its middle frame)
//   lang    en | ru

import { createContext, useContext, useRef, useState } from "react";
import { Face, faceWidth } from "./Face";
import { faceStrings, presetTitle } from "./i18n";
import type { Vec } from "./motion";
import { FACE_STATES, IDLE_EMOTES, PRESETS, presetFace, type FaceState, type IdleEmote } from "./schema";
import { useFakeVoice, useGazeToward, useWindowPointer, type PointerPos } from "./useGaze";
import { I18nContext, stringsFor } from "../../i18n";
import { FaceEditor } from "./FaceEditor";
import type { FaceConfig } from "./schema";
import "./face.css";

const params = new URLSearchParams(window.location.search);
const lang = params.get("lang") === "ru" ? "ru" : "en";
const t = faceStrings(lang);
const fixedGaze: Vec | null = (() => {
  const raw = params.get("gaze");
  if (!raw) return null;
  const [x, y] = raw.split(",").map(Number);
  return Number.isFinite(x) && Number.isFinite(y) ? { x: x!, y: y! } : null;
})();
const paramEmote = IDLE_EMOTES.find((e) => e === params.get("emote")) ?? null;

const Pointer = createContext<PointerPos | null>(null);

function useGazeFor(ref: React.RefObject<HTMLElement | null>): Vec | null {
  return useGazeToward(ref, useContext(Pointer), 360, fixedGaze);
}

function Pill(props: { preset: string; state: FaceState; size: number; still: boolean; reduced: boolean; level: number; emote?: { kind: IdleEmote; nonce: number } | null; label?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const gaze = useGazeFor(ref);
  const compact = props.size < 64;
  const w = faceWidth(props.size, compact);
  return (
    <div className="fd-cell">
      <div
        ref={ref}
        className={compact ? "face-pill" : "face-card"}
        style={compact ? { height: props.size, width: w + props.size * 1.4, borderRadius: `0 0 ${props.size * 0.42}px ${props.size * 0.42}px` } : { width: w + 24, height: props.size + 24 }}
      >
        <Face
          config={presetFace(props.preset)}
          state={props.state}
          gaze={gaze}
          size={props.size}
          still={props.still}
          reducedMotion={props.reduced}
          level={props.level}
          emote={props.emote}
          autoSleep={false}
        />
      </div>
      {props.label ? <div className="fd-label">{props.label}</div> : null}
    </div>
  );
}

function Stage(props: { still: boolean; reduced: boolean }) {
  const [preset, setPreset] = useState(params.get("preset") ?? "mia");
  const [state, setState] = useState<FaceState>((FACE_STATES.find((s) => s === params.get("state")) as FaceState) ?? "idle");
  const [voice, setVoice] = useState(false);
  const [manual, setManual] = useState(0.5);
  const fake = useFakeVoice(voice);
  const level = voice ? fake : manual;
  const [emote, setEmote] = useState<{ kind: IdleEmote; nonce: number } | null>(null);
  const p = PRESETS.find((x) => x.name === preset);

  return (
    <section className="fd-stage">
      <div className="fd-screen">
        <div className="fd-menubar">
          <Pill preset={preset} state={state} size={34} still={props.still} reduced={props.reduced} level={level} emote={emote} />
        </div>
        <div className="fd-open">
          <div className="fd-open-face">
            <Pill preset={preset} state={state} size={132} still={props.still} reduced={props.reduced} level={level} emote={emote} />
          </div>
          <div className="fd-open-text">
            <div className="fd-open-name">{p ? presetTitle(p, lang) : preset}</div>
            <div className="fd-open-state">{t.states[state]}</div>
          </div>
        </div>
      </div>
      <div className="fd-controls">
        <label>
          {t.demo.preset}{" "}
          <select className="select" value={preset} onChange={(e) => setPreset(e.target.value)}>
            {PRESETS.map((x) => (
              <option key={x.name} value={x.name}>
                {presetTitle(x, lang)}
              </option>
            ))}
          </select>
        </label>
        <div className="fd-chips">
          {FACE_STATES.map((s) => (
            <button key={s} type="button" className="chip" aria-pressed={s === state} onClick={() => setState(s)}>
              {t.states[s]}
            </button>
          ))}
        </div>
        <div className="fd-chips">
          {IDLE_EMOTES.map((e) => (
            <button
              key={e}
              type="button"
              className="chip"
              onClick={() => {
                setState("idle");
                setEmote({ kind: e, nonce: Date.now() });
              }}
            >
              {t.emotes[e]}
            </button>
          ))}
        </div>
        {state === "listening" ? (
          <div className="fd-chips">
            <button type="button" className="chip" aria-pressed={voice} onClick={() => setVoice(!voice)}>
              {t.demo.talk}
            </button>
            {!voice ? (
              <label>
                {t.demo.level}{" "}
                <input className="face-range" type="range" min={0} max={1} step={0.01} value={manual} onChange={(e) => setManual(Number(e.target.value))} />
              </label>
            ) : null}
          </div>
        ) : null}
      </div>
    </section>
  );
}

function Grid(props: { view: string; size: number; still: boolean; reduced: boolean }) {
  const { view, size, still, reduced } = props;
  const preset = params.get("preset") ?? "mia";
  const state = (FACE_STATES.find((s) => s === params.get("state")) as FaceState) ?? "idle";
  const emote = paramEmote ? { kind: paramEmote, nonce: 1 } : null;
  if (view === "presets") {
    return (
      <div className="fd-row">
        {PRESETS.map((p) => (
          <Pill key={p.name} preset={p.name} state={state} size={size} still={still} reduced={reduced} level={0.6} emote={emote} label={presetTitle(p, lang)} />
        ))}
      </div>
    );
  }
  if (view === "states") {
    return (
      <div className="fd-row">
        {FACE_STATES.map((s) => (
          <Pill key={s} preset={preset} state={s} size={size} still={still} reduced={reduced} level={0.6} label={t.states[s]} />
        ))}
      </div>
    );
  }
  return (
    <table className="fd-grid">
      <thead>
        <tr>
          <th />
          {FACE_STATES.map((s) => (
            <th key={s}>{t.states[s]}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {PRESETS.map((p) => (
          <tr key={p.name}>
            <th>{presetTitle(p, lang)}</th>
            {FACE_STATES.map((s) => (
              <td key={s}>
                <Pill preset={p.name} state={s} size={size} still={still} reduced={reduced} level={0.6} />
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function EditorView() {
  const [face, setFace] = useState<FaceConfig | null>(params.get("preset") ? presetFace(params.get("preset")!) : null);
  return (
    <I18nContext.Provider value={{ lang, s: stringsFor(lang) }}>
      <div className="mock-window fd-editor">
        <div className="main">
          <FaceEditor value={face} synthId="synth_demo" onChange={setFace} />
        </div>
      </div>
    </I18nContext.Provider>
  );
}

export function FaceDemo() {
  const pointer = useWindowPointer();
  const view = params.get("view") ?? "stage";
  const [still, setStill] = useState(params.get("still") === "1");
  const [reduced, setReduced] = useState(params.get("reduced") === "1");
  const size = Number(params.get("size")) || 96;

  return (
    <Pointer.Provider value={pointer}>
      <div className={`fd ${view === "stage" ? "" : "fd-bare"}`}>
        {view === "editor" ? (
          <EditorView />
        ) : view === "stage" ? (
          <>
            <header className="fd-head">
              <h1 className="page-title">{t.demo.title}</h1>
              <p className="page-lead">{t.demo.lead}</p>
              <div className="fd-chips">
                <button type="button" className="chip" aria-pressed={!still} onClick={() => setStill(!still)}>
                  {still ? t.demo.still : t.demo.live}
                </button>
                <button type="button" className="chip" aria-pressed={reduced} onClick={() => setReduced(!reduced)}>
                  {t.demo.reduced}
                </button>
              </div>
            </header>
            <Stage still={still} reduced={reduced} />
            <h2 className="section-title fd-gridtitle">{t.demo.grid}</h2>
            <Grid view="grid" size={56} still={still} reduced={reduced} />
          </>
        ) : (
          <Grid view={view} size={size} still={still} reduced={reduced} />
        )}
      </div>
    </Pointer.Provider>
  );
}
