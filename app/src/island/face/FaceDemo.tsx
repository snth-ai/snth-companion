// Review page for the face (browser only, mock mode). Open
// http://127.0.0.1:1420/src/island/face/demo.html while `npm run dev` runs.
//
// URL parameters:
//   view    stage (default: the island in a menu bar, closed and open,
//           then the grid), notch (every state in the measured notch
//           fixture at 32/34/37 px, light and dark, notch and no notch),
//           grid (every look x every state), presets (every look in one
//           state), states (one look in every state), editor (the face
//           editor in a frame of the app window), one (a single face)
//   size    face height in px for grid / presets / states / one (default
//           96; below 48 the faces sit in the notch fixture)
//   preset  the look for stage / states / notch / one (default mia)
//   state   the state for presets / one (default idle)
//   still   1 = settled poses, no animation (screenshots)
//   gaze    x,y fixed gaze for every face, e.g. 0.4,0.6 (screenshots)
//   emote   play this idle emote (with still=1: its middle frame)
//   level   microphone level for listening (default 0.6)
//   reduced 1 = reduced motion
//   perf=1  counts animation frames into the page title (energy checks)
//   calm    1 = no glances or habits, 2 = no blinks either (energy checks)
//   lang    en | ru

import { createContext, useContext, useRef, useState } from "react";
import { I18nContext, stringsFor } from "../../i18n";
import { Face, faceWidth } from "./Face";
import { FaceEditor } from "./FaceEditor";
import { NotchFixture } from "./Fixture";
import { faceStrings, presetTitle } from "./i18n";
import type { Vec } from "./motion";
import { FACE_STATES, IDLE_EMOTES, PRESETS, presetFace, type FaceConfig, type FaceState, type IdleEmote } from "./schema";
import { useFakeVoice, useGazeToward, useWindowPointer, type PointerPos } from "./useGaze";
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
const paramState = (FACE_STATES.find((s) => s === params.get("state")) as FaceState | undefined) ?? "idle";
const paramPreset = params.get("preset") ?? "mia";
const paramLevel = params.get("level") === null ? 0.6 : Number(params.get("level"));

/** ?calm=1 (no glances, no habits) and ?calm=2 (no blinks either): energy checks. */
function calm(face: FaceConfig): FaceConfig {
  const level = params.get("calm");
  if (!level) return face;
  return { ...face, idle_emotes: [], motion: { ...face.motion, liveliness: 0, blink: level === "2" ? 0 : face.motion.blink } };
}

const look = (name: string) => calm(presetFace(name));

const Pointer = createContext<PointerPos | null>(null);

function useGazeFor(ref: React.RefObject<HTMLElement | null>): Vec | null {
  return useGazeToward(ref, useContext(Pointer), 360, fixedGaze);
}

interface CellProps {
  preset: string;
  state: FaceState;
  size: number;
  still: boolean;
  reduced: boolean;
  level: number;
  emote?: { kind: IdleEmote; nonce: number } | null;
  label?: string;
}

/** One face: below 48 px in the measured notch fixture, above on a card. */
function Cell(props: CellProps) {
  const ref = useRef<HTMLDivElement>(null);
  const gaze = useGazeFor(ref);
  const closed = props.size < 48;
  return (
    <div className="fd-cell" ref={ref}>
      {closed ? (
        <NotchFixture
          face={look(props.preset)}
          state={props.state}
          height={props.size}
          gaze={gaze}
          level={props.level}
          still={props.still}
          reduced={props.reduced}
          emote={props.emote}
          barWidth={1}
        />
      ) : (
        <div className="face-card" style={{ width: faceWidth(props.size) + 24, height: props.size + 24 }}>
          <Face
            config={look(props.preset)}
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
      )}
      {props.label ? <div className="fd-label">{props.label}</div> : null}
    </div>
  );
}

function Stage(props: { still: boolean; reduced: boolean }) {
  const [preset, setPreset] = useState(paramPreset);
  const [state, setState] = useState<FaceState>(paramState);
  const [voice, setVoice] = useState(false);
  const [manual, setManual] = useState(0.5);
  const [light, setLight] = useState(false);
  const [height, setHeight] = useState(37);
  const fake = useFakeVoice(voice);
  const level = voice ? fake : manual;
  const [emote, setEmote] = useState<{ kind: IdleEmote; nonce: number } | null>(null);
  const p = PRESETS.find((x) => x.name === preset);
  const face = look(preset);
  const barRef = useRef<HTMLDivElement>(null);
  const openRef = useRef<HTMLDivElement>(null);
  const barGaze = useGazeFor(barRef);
  const openGaze = useGazeFor(openRef);

  return (
    <section className="fd-stage">
      <div className={`fd-screen${light ? " light" : ""}`}>
        <div ref={barRef}>
          <NotchFixture face={face} state={state} height={height} light={light} gaze={barGaze} level={level} still={props.still} reduced={props.reduced} emote={emote} barWidth={620} />
        </div>
        <div className="fd-open" ref={openRef}>
          <Face config={face} state={state} gaze={openGaze} size={84} still={props.still} reducedMotion={props.reduced} level={level} emote={emote} autoSleep={false} />
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
          {[32, 34, 37].map((h) => (
            <button key={h} type="button" className="chip" aria-pressed={h === height} onClick={() => setHeight(h)}>
              {h} px
            </button>
          ))}
          <button type="button" className="chip" aria-pressed={light} onClick={() => setLight(!light)}>
            {t.demo.lightBar}
          </button>
        </div>
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

/** Every state in the measured fixture: 32 / 34 / 37 px, dark and light, with and without a notch. */
function NotchMatrix(props: { still: boolean; reduced: boolean }) {
  const face = look(paramPreset);
  const variants: { h: number; light: boolean; notch: boolean; label: string }[] = [
    { h: 32, light: false, notch: true, label: "32 px" },
    { h: 34, light: true, notch: true, label: "34 px, light" },
    { h: 37, light: false, notch: true, label: "37 px" },
    { h: 34, light: true, notch: false, label: "34 px, no notch" },
  ];
  return (
    <table className="fd-grid fd-notch">
      <thead>
        <tr>
          <th />
          {variants.map((v) => (
            <th key={v.label}>{v.label}</th>
          ))}
        </tr>
      </thead>
      <tbody>
        {FACE_STATES.map((s) => (
          <tr key={s}>
            <th>{t.states[s]}</th>
            {variants.map((v) => (
              <td key={v.label}>
                <NotchFixture face={face} state={s} height={v.h} light={v.light} notch={v.notch} gaze={fixedGaze} level={paramLevel} still={props.still} reduced={props.reduced} guides />
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function Grid(props: { view: string; size: number; still: boolean; reduced: boolean }) {
  const { view, size, still, reduced } = props;
  const emote = paramEmote ? { kind: paramEmote, nonce: 1 } : null;
  if (view === "one") {
    return <Cell preset={paramPreset} state={paramState} size={size} still={still} reduced={reduced} level={paramLevel} emote={emote} />;
  }
  if (view === "presets") {
    return (
      <div className="fd-row">
        {PRESETS.map((p) => (
          <Cell key={p.name} preset={p.name} state={paramState} size={size} still={still} reduced={reduced} level={paramLevel} emote={emote} label={presetTitle(p, lang)} />
        ))}
      </div>
    );
  }
  if (view === "states") {
    return (
      <div className="fd-row">
        {FACE_STATES.map((s) => (
          <Cell key={s} preset={paramPreset} state={s} size={size} still={still} reduced={reduced} level={paramLevel} label={t.states[s]} />
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
                <Cell preset={p.name} state={s} size={size} still={still} reduced={reduced} level={paramLevel} />
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
      <div className={`mock-window fd-editor${params.get("wide") === "1" ? " wide" : ""}`}>
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

  let body;
  if (view === "editor") body = <EditorView />;
  else if (view === "notch") body = <NotchMatrix still={still} reduced={reduced} />;
  else if (view === "stage")
    body = (
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
    );
  else body = <Grid view={view} size={size} still={still} reduced={reduced} />;

  return (
    <Pointer.Provider value={pointer}>
      <div className={`fd ${view === "stage" ? "" : "fd-bare"}`}>{body}</div>
    </Pointer.Provider>
  );
}
