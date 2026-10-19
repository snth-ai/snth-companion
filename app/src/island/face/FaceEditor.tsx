// The face editor. A preview stays in view while editing: the closed
// island at its real size (in the left ear of a measured notch) and the
// open island, with the state to show and Undo / Redo. Then the looks as a
// grid and grouped controls. Emits a full, valid FaceConfig on every change.
//
// Layout: a pinned strip on top in a narrow window (the app's 760 px window),
// a sticky left column once the editor is 760 px wide or more.

import { useEffect, useMemo, useReducer, useRef, useState, type KeyboardEvent, type ReactNode } from "react";
import { useI18n } from "../../i18n";
import { Row } from "../../ui/controls";
import { Face } from "./Face";
import { NotchFixture } from "./Fixture";
import { eyeGeom, heart, softStar } from "./geometry";
import { faceStrings, presetBlurb, presetTitle, type FaceStrings } from "./i18n";
import { randomFace } from "./random";
import {
  ACCESSORIES,
  BROW_STYLES,
  EYE_SHAPES,
  FACE_STATES,
  IDLE_EMOTES,
  LASHES,
  MAX_SLEEPY_MIN,
  MOUTH_STYLES,
  PRESETS,
  PUPIL_SHAPES,
  clampFace,
  deepMerge,
  defaultFaceFor,
  findPreset,
  presetFace,
  type FaceConfig,
  type FaceState,
  type IdleEmote,
  type PupilShape,
} from "./schema";
import { useFakeVoice, useGazeToward, useWindowPointer } from "./useGaze";
import "./face.css";

/** A deep partial of FaceConfig, for one control's change. */
type Patch = { [K in keyof FaceConfig]?: FaceConfig[K] extends object ? Patch2<FaceConfig[K]> : FaceConfig[K] };
type Patch2<T> = T extends unknown[] ? T : { [K in keyof T]?: T[K] extends object ? Patch2<T[K]> : T[K] };

export interface FaceEditorProps {
  /** The current look; null = the synth has none yet (its default is shown). */
  value: FaceConfig | null | undefined;
  /** Picks the default look when `value` is null, and for Restore. */
  synthId?: string;
  onChange: (face: FaceConfig) => void;
  /** Defaults to the app's language. */
  lang?: "en" | "ru";
}

/** The closed island height the preview uses (a 14" MacBook Pro menu bar is 37). */
const NATIVE = 34;
const HISTORY = 50;
/** Changes to the same control within this window are one undo step. */
const COALESCE_MS = 1200;

interface History {
  past: FaceConfig[];
  future: FaceConfig[];
  key: string | null;
  at: number;
}

export function FaceEditor(props: FaceEditorProps) {
  const app = useI18n();
  const lang = props.lang ?? app.lang;
  const t = faceStrings(lang);
  const fallback = useMemo(() => defaultFaceFor(props.synthId ?? ""), [props.synthId]);
  const face = useMemo(() => (props.value ? clampFace(props.value, fallback) : fallback), [props.value, fallback]);
  const [state, setState] = useState<FaceState>("idle");
  const [emote, setEmote] = useState<{ kind: IdleEmote; nonce: number } | null>(null);

  const hist = useRef<History>({ past: [], future: [], key: null, at: 0 });
  const [, rerender] = useReducer((x: number) => x + 1, 0);

  const base = findPreset(face.preset);
  const changed = base ? JSON.stringify(base.face) !== JSON.stringify(face) : true;

  /** Every change goes through here so it can be undone. `key` names the
   * control: a drag on one slider is one step. */
  const commit = (next: FaceConfig, key?: string) => {
    const h = hist.current;
    const now = Date.now();
    const same = key !== undefined && h.key === key && now - h.at < COALESCE_MS;
    if (!same) {
      h.past.push(face);
      if (h.past.length > HISTORY) h.past.shift();
    }
    h.future = [];
    h.key = key ?? null;
    h.at = now;
    props.onChange(next);
    rerender();
  };
  const endDrag = () => {
    hist.current.key = null;
  };
  const undo = () => {
    const h = hist.current;
    const prev = h.past.pop();
    if (!prev) return;
    h.future.push(face);
    h.key = null;
    props.onChange(prev);
    rerender();
  };
  const redo = () => {
    const h = hist.current;
    const next = h.future.pop();
    if (!next) return;
    h.past.push(face);
    h.key = null;
    props.onChange(next);
    rerender();
  };
  const set = (patch: Patch, key: string) => commit(clampFace(deepMerge(face, patch), face), key);
  const play = (kind: IdleEmote) => {
    setState("idle");
    setEmote({ kind, nonce: Date.now() });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!(e.metaKey || e.ctrlKey) || e.key.toLowerCase() !== "z") return;
    const target = e.target as HTMLElement;
    if (target.tagName === "INPUT" && (target as HTMLInputElement).type === "text") return;
    e.preventDefault();
    if (e.shiftKey) redo();
    else undo();
  };

  const f = t.editor.fields;
  const pct = (v: number) => t.editor.percent(Math.round(v * 100));

  return (
    <div className="fe" onKeyDown={onKeyDown} onPointerUp={endDrag}>
      <div className="fe-layout">
        <Preview
          face={face}
          state={state}
          setState={setState}
          emote={emote}
          t={t}
          canUndo={hist.current.past.length > 0}
          canRedo={hist.current.future.length > 0}
          undo={undo}
          redo={redo}
        />

        <div className="fe-body">
          <section className="section">
            <h2 className="section-title">{t.editor.presets}</h2>
            <div className="fe-looks" role="radiogroup" aria-label={t.editor.presets}>
              {PRESETS.map((p) => {
                const on = face.preset === p.name;
                return (
                  <button
                    key={p.name}
                    type="button"
                    role="radio"
                    aria-checked={on}
                    className="fe-look"
                    title={presetBlurb(p, lang)}
                    onClick={() => commit(presetFace(p.name))}
                  >
                    <span className="fe-look-face">
                      <Face config={p.face} size={56} still />
                    </span>
                    <span className="fe-look-name">
                      {presetTitle(p, lang)}
                      {on && changed ? <span className="tag">{t.editor.custom}</span> : null}
                    </span>
                  </button>
                );
              })}
            </div>
            <p className="section-note">{base ? presetBlurb(base, lang) : t.editor.presetsNote}</p>
            <div className="fe-actions">
              <button
                type="button"
                className="btn"
                title={t.editor.resetHint}
                disabled={!changed && !!base}
                onClick={() => commit(base ? presetFace(base.name) : fallback)}
              >
                {t.editor.reset}
              </button>
              <button type="button" className="btn" onClick={() => commit(randomFace())}>
                {t.editor.random}
              </button>
            </div>
          </section>

          <Group title={t.editor.groups.eyes} open>
            <Row title={f.shape} desc={t.shapes[face.eyes.shape]}>
              <Glyphs
                label={f.shape}
                value={face.eyes.shape}
                options={EYE_SHAPES.map((s) => ({ value: s, title: t.shapes[s], glyph: <EyeGlyph shape={s} /> }))}
                onChange={(shape) => set({ eyes: { shape } }, "eyes.shape")}
              />
            </Row>
            <Slider title={f.size} value={face.eyes.size} format={pct} onChange={(size) => set({ eyes: { size } }, "eyes.size")} />
            <Slider title={f.spacing} value={face.eyes.spacing} format={pct} onChange={(spacing) => set({ eyes: { spacing } }, "eyes.spacing")} />
            <Slider
              title={f.tilt}
              value={face.eyes.tilt}
              min={-1}
              format={(v) => t.editor.degrees(Math.round(v * 16))}
              onChange={(tilt) => set({ eyes: { tilt } }, "eyes.tilt")}
            />
            <Colour title={f.sclera} value={face.eyes.sclera} t={t} onChange={(sclera) => set({ eyes: { sclera } }, "eyes.sclera")} />
            <Slider title={f.shine} value={face.eyes.shine} format={pct} onChange={(shine) => set({ eyes: { shine } }, "eyes.shine")} />
            <Slider title={f.glow} value={face.eyes.glow} format={pct} onChange={(glow) => set({ eyes: { glow } }, "eyes.glow")} />
          </Group>

          <Group title={t.editor.groups.iris}>
            <Colour title={f.irisColor} value={face.eyes.iris.color} t={t} onChange={(color) => set({ eyes: { iris: { color } } }, "iris.color")} />
            <Slider title={f.irisSize} value={face.eyes.iris.size} format={pct} onChange={(size) => set({ eyes: { iris: { size } } }, "iris.size")} />
            <Row title={f.pupilShape} desc={t.pupils[face.eyes.pupil.shape]}>
              <Glyphs
                label={f.pupilShape}
                value={face.eyes.pupil.shape}
                options={PUPIL_SHAPES.map((s) => ({ value: s, title: t.pupils[s], glyph: <PupilGlyph shape={s} /> }))}
                onChange={(shape) => set({ eyes: { pupil: { shape } } }, "pupil.shape")}
              />
            </Row>
            <Colour title={f.pupilColor} value={face.eyes.pupil.color} t={t} onChange={(color) => set({ eyes: { pupil: { color } } }, "pupil.color")} />
            <Slider title={f.pupilSize} value={face.eyes.pupil.size} format={pct} onChange={(size) => set({ eyes: { pupil: { size } } }, "pupil.size")} />
          </Group>

          <Group title={t.editor.groups.lids}>
            <Slider title={f.lidRest} value={face.eyes.lids.rest} format={pct} onChange={(rest) => set({ eyes: { lids: { rest } } }, "lids.rest")} />
            <Colour title={f.lidColor} value={face.eyes.lids.color} t={t} onChange={(color) => set({ eyes: { lids: { color } } }, "lids.color")} />
            <Choice title={f.lashes} value={face.eyes.lashes} options={LASHES} labels={t.lashes} onChange={(lashes) => set({ eyes: { lashes } }, "lashes")} />
          </Group>

          <Group title={t.editor.groups.brows}>
            <Choice
              title={f.style}
              value={face.brows.style}
              options={BROW_STYLES}
              labels={t.brows}
              onChange={(style) => set({ brows: { style } }, "brows.style")}
            />
            <Colour title={f.color} value={face.brows.color} t={t} onChange={(color) => set({ brows: { color } }, "brows.color")} />
          </Group>

          <Group title={t.editor.groups.mouth}>
            <Choice
              title={f.style}
              value={face.mouth.style}
              options={MOUTH_STYLES}
              labels={t.mouths}
              onChange={(style) => set({ mouth: { style } }, "mouth.style")}
            />
            <Colour title={f.color} value={face.mouth.color} t={t} onChange={(color) => set({ mouth: { color } }, "mouth.color")} />
          </Group>

          <Group title={t.editor.groups.cheeks}>
            <Slider title={f.blush} value={face.cheeks.blush} format={pct} onChange={(blush) => set({ cheeks: { blush } }, "cheeks.blush")} />
            <Colour title={f.color} value={face.cheeks.color} t={t} onChange={(color) => set({ cheeks: { color } }, "cheeks.color")} />
          </Group>

          <Group title={t.editor.groups.accessory} open>
            <Choice
              title={f.accessory}
              value={face.accessory}
              options={ACCESSORIES}
              labels={t.accessories}
              onChange={(accessory) => set({ accessory }, "accessory")}
            />
            <Colour title={f.color} value={face.accessory_color} t={t} onChange={(accessory_color) => set({ accessory_color }, "accessory_color")} />
          </Group>

          <Group title={t.editor.groups.color}>
            <Colour title={f.accent} desc={f.accentDesc} value={face.palette.accent} t={t} onChange={(accent) => set({ palette: { accent } }, "accent")} />
          </Group>

          <Group title={t.editor.groups.motion} open>
            <Slider title={f.blink} value={face.motion.blink} format={pct} onChange={(blink) => set({ motion: { blink } }, "motion.blink")} />
            <Slider
              title={f.curiosity}
              desc={f.curiosityDesc}
              value={face.motion.curiosity}
              format={pct}
              onChange={(curiosity) => set({ motion: { curiosity } }, "motion.curiosity")}
            />
            <Slider
              title={f.liveliness}
              desc={f.livelinessDesc}
              value={face.motion.liveliness}
              format={pct}
              onChange={(liveliness) => set({ motion: { liveliness } }, "motion.liveliness")}
            />
            <Slider
              title={f.sleepy}
              value={face.motion.sleepy_after_min}
              min={0}
              max={MAX_SLEEPY_MIN}
              step={1}
              format={(n) => (n === 0 ? f.sleepyNever : f.minutes(n))}
              onChange={(sleepy_after_min) => set({ motion: { sleepy_after_min } }, "motion.sleepy")}
            />
            <Row title={f.emotes} desc={f.emotesDesc}>
              <span />
            </Row>
            <div className="fe-emotes">
              {IDLE_EMOTES.map((e) => {
                const on = face.idle_emotes.includes(e);
                return (
                  <span key={e} className="fe-habit">
                    <button
                      type="button"
                      className="chip"
                      aria-pressed={on}
                      onClick={() => {
                        const next = on ? face.idle_emotes.filter((x) => x !== e) : [...face.idle_emotes, e];
                        commit(clampFace({ ...face, idle_emotes: next }, face));
                      }}
                    >
                      {t.emotes[e]}
                    </button>
                    <button
                      type="button"
                      className="btn quiet icon-only fe-play"
                      aria-label={t.editor.previewHabit(t.emotes[e])}
                      title={t.editor.previewHabit(t.emotes[e])}
                      onClick={() => play(e)}
                    >
                      <svg viewBox="0 0 12 12" width={11} height={11} aria-hidden="true">
                        <path d="M3.5 2.2L9.6 6L3.5 9.8Z" fill="currentColor" />
                      </svg>
                    </button>
                  </span>
                );
              })}
            </div>
          </Group>
        </div>
      </div>
    </div>
  );
}

function Preview(props: {
  face: FaceConfig;
  state: FaceState;
  setState: (s: FaceState) => void;
  emote: { kind: IdleEmote; nonce: number } | null;
  t: FaceStrings;
  canUndo: boolean;
  canRedo: boolean;
  undo: () => void;
  redo: () => void;
}) {
  const { t } = props;
  const pointer = useWindowPointer();
  const level = useFakeVoice(props.state === "listening");
  const small = useRef<HTMLDivElement>(null);
  const big = useRef<HTMLDivElement>(null);
  const smallGaze = useGazeToward(small, pointer);
  const bigGaze = useGazeToward(big, pointer);
  return (
    <div className="fe-preview" aria-label={t.editor.preview}>
      <div className="fe-preview-stage">
        <div className="fe-preview-closed" ref={small}>
          <NotchFixture face={props.face} state={props.state} height={NATIVE} gaze={smallGaze} level={level} emote={props.emote} crop barWidth={1} />
          <span className="fe-size">{t.editor.actualSize(NATIVE)}</span>
        </div>
        <div className="fe-preview-open" ref={big} aria-label={t.editor.openSize}>
          <Face config={props.face} state={props.state} gaze={bigGaze} level={level} size={84} emote={props.emote} autoSleep={false} />
        </div>
      </div>
      <div className="fe-preview-bar">
        <select className="select" value={props.state} aria-label={t.editor.stateLabel} onChange={(e) => props.setState(e.target.value as FaceState)}>
          {FACE_STATES.map((s) => (
            <option key={s} value={s}>
              {t.states[s]}
            </option>
          ))}
        </select>
        <span className="fe-preview-gap" />
        <button type="button" className="btn small" disabled={!props.canUndo} onClick={props.undo} title={`${t.editor.undo} (⌘Z)`}>
          {t.editor.undo}
        </button>
        <button type="button" className="btn small" disabled={!props.canRedo} onClick={props.redo} title={`${t.editor.redo} (⇧⌘Z)`}>
          {t.editor.redo}
        </button>
      </div>
    </div>
  );
}

function Group(props: { title: string; open?: boolean; children: ReactNode }) {
  return (
    <details className="fe-group section" open={props.open}>
      <summary className="section-title fe-summary">{props.title}</summary>
      <div className="group">{props.children}</div>
    </details>
  );
}

function Slider(props: {
  title: string;
  desc?: ReactNode;
  value: number;
  min?: number;
  max?: number;
  step?: number;
  format: (n: number) => string;
  onChange: (n: number) => void;
}) {
  const { min = 0, max = 1, step = 0.01 } = props;
  const shown = props.format(props.value);
  return (
    <Row title={props.title} desc={props.desc}>
      <span className="fe-slider">
        <input
          className="face-range"
          type="range"
          min={min}
          max={max}
          step={step}
          value={props.value}
          aria-label={props.title}
          aria-valuetext={shown}
          onChange={(e) => props.onChange(Number(e.target.value))}
        />
        <span className="fe-num">{shown}</span>
      </span>
    </Row>
  );
}

const HEX = /^#?([0-9a-fA-F]{6})$/;

function Colour(props: { title: string; desc?: ReactNode; value: string; t: FaceStrings; onChange: (hex: string) => void }) {
  const [text, setText] = useState(props.value.toUpperCase());
  useEffect(() => setText(props.value.toUpperCase()), [props.value]);
  const apply = () => {
    const m = HEX.exec(text.trim());
    if (m) props.onChange(`#${m[1]!.toUpperCase()}`);
    else setText(props.value.toUpperCase());
  };
  return (
    <Row title={props.title} desc={props.desc}>
      <span className="fe-colour">
        <input
          className="input fe-hex"
          type="text"
          value={text}
          maxLength={7}
          spellCheck={false}
          aria-label={props.t.editor.hexLabel(props.title)}
          aria-invalid={!HEX.test(text.trim())}
          onChange={(e) => setText(e.target.value)}
          onBlur={apply}
          onKeyDown={(e) => {
            if (e.key === "Enter") apply();
          }}
        />
        <input type="color" value={props.value.toLowerCase()} aria-label={props.title} onChange={(e) => props.onChange(e.target.value.toUpperCase())} />
      </span>
    </Row>
  );
}

function Choice<T extends string>(props: { title: string; value: T; options: readonly T[]; labels: Record<T, string>; onChange: (v: T) => void }) {
  return (
    <Row title={props.title}>
      <select className="select" value={props.value} aria-label={props.title} onChange={(e) => props.onChange(e.target.value as T)}>
        {props.options.map((o) => (
          <option key={o} value={o}>
            {props.labels[o]}
          </option>
        ))}
      </select>
    </Row>
  );
}

function Glyphs<T extends string>(props: { label: string; value: T; options: { value: T; title: string; glyph: ReactNode }[]; onChange: (v: T) => void }) {
  return (
    <div className="swatches fe-glyphs" role="radiogroup" aria-label={props.label}>
      {props.options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          className="swatch"
          aria-checked={o.value === props.value}
          title={o.title}
          aria-label={o.title}
          onClick={() => props.onChange(o.value)}
        >
          {o.glyph}
        </button>
      ))}
    </div>
  );
}

function EyeGlyph(props: { shape: FaceConfig["eyes"]["shape"] }) {
  const g = eyeGeom(props.shape, 10);
  return (
    <svg viewBox="-13 -13 26 26" width={18} height={18} aria-hidden="true">
      <path d={g.path} fill="currentColor" />
    </svg>
  );
}

function PupilGlyph(props: { shape: PupilShape }) {
  const r = 5.2;
  const body =
    props.shape === "round" ? (
      <circle r={r} />
    ) : props.shape === "slit" ? (
      <ellipse rx={r * 0.35} ry={r * 1.8} />
    ) : props.shape === "star" ? (
      <path d={softStar(0, 0.3, r * 1.6, r * 0.7, 5, 0.22)} />
    ) : props.shape === "heart" ? (
      <path d={heart(0, 0, r * 1.4)} />
    ) : (
      <>
        <circle r={r * 1.1} fill="none" stroke="currentColor" strokeWidth={r * 0.5} />
        <circle r={r * 0.25} />
      </>
    );
  return (
    <svg viewBox="-12 -12 24 24" width={18} height={18} aria-hidden="true" fill="currentColor">
      {body}
    </svg>
  );
}
