// The face editor: a live preview (closed and open island), the looks as a
// grid, then grouped controls. Emits a full, valid FaceConfig on every change.

import { useMemo, useRef, useState, type ReactNode } from "react";
import { useI18n } from "../../i18n";
import { Row, Section } from "../../ui/controls";
import { Face } from "./Face";
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
import { useFakeVoice, useGazeToward, useWindowPointer, type PointerPos } from "./useGaze";
import "./face.css";

/** A deep partial of FaceConfig, for one control's change. */
type Patch = { [K in keyof FaceConfig]?: FaceConfig[K] extends object ? Patch2<FaceConfig[K]> : FaceConfig[K] };
type Patch2<T> = T extends unknown[] ? T : { [K in keyof T]?: T[K] extends object ? Patch2<T[K]> : T[K] };

export interface FaceEditorProps {
  /** The current look; null = the synth has none yet (its default is shown). */
  value: FaceConfig | null | undefined;
  /** Picks the default look when `value` is null, and for Reset. */
  synthId?: string;
  onChange: (face: FaceConfig) => void;
  /** Defaults to the app's language. */
  lang?: "en" | "ru";
}

export function FaceEditor(props: FaceEditorProps) {
  const app = useI18n();
  const lang = props.lang ?? app.lang;
  const t = faceStrings(lang);
  const fallback = useMemo(() => defaultFaceFor(props.synthId ?? ""), [props.synthId]);
  const face = useMemo(() => (props.value ? clampFace(props.value, fallback) : fallback), [props.value, fallback]);
  const [state, setState] = useState<FaceState>("idle");
  const [emote, setEmote] = useState<{ kind: IdleEmote; nonce: number } | null>(null);
  const pointer = useWindowPointer();
  const level = useFakeVoice(state === "listening");

  const base = findPreset(face.preset);
  const changed = base ? JSON.stringify(base.face) !== JSON.stringify(face) : true;

  const set = (patch: Patch) => props.onChange(clampFace(deepMerge(face, patch), face));
  const play = (kind: IdleEmote) => {
    setState("idle");
    setEmote({ kind, nonce: Date.now() });
  };

  return (
    <div className="fe">
      <Section title={t.editor.preview} note={t.editor.previewNote}>
        <Preview face={face} state={state} level={level} emote={emote} pointer={pointer} t={t} />
        <div className="fe-states" role="group" aria-label={t.editor.stateLabel}>
          {FACE_STATES.map((s) => (
            <button key={s} type="button" className="chip" aria-pressed={s === state} onClick={() => setState(s)}>
              {t.states[s]}
            </button>
          ))}
        </div>
      </Section>

      <Section title={t.editor.presets} note={base ? presetBlurb(base, lang) : t.editor.presetsNote}>
        <div className="fe-looks" role="radiogroup" aria-label={t.editor.presets}>
          {PRESETS.map((p) => {
            const on = face.preset === p.name;
            return (
              <button key={p.name} type="button" role="radio" aria-checked={on} className="fe-look" title={presetBlurb(p, lang)} onClick={() => props.onChange(presetFace(p.name))}>
                <span className="fe-look-face">
                  <Face config={p.face} size={50} compact={false} still />
                </span>
                <span className="fe-look-name">
                  {presetTitle(p, lang)}
                  {on && changed ? <span className="tag">{t.editor.custom}</span> : null}
                </span>
              </button>
            );
          })}
        </div>
        <div className="fe-actions">
          <button
            type="button"
            className="btn"
            title={t.editor.resetHint}
            disabled={!changed && !!base}
            onClick={() => props.onChange(base ? presetFace(base.name) : fallback)}
          >
            {t.editor.reset}
          </button>
          <button type="button" className="btn" onClick={() => props.onChange(randomFace())}>
            {t.editor.random}
          </button>
        </div>
      </Section>

      <Section title={t.editor.groups.eyes}>
        <div className="group">
          <Row title={t.editor.fields.shape}>
            <Glyphs
              label={t.editor.fields.shape}
              value={face.eyes.shape}
              options={EYE_SHAPES.map((s) => ({ value: s, title: t.shapes[s], glyph: <EyeGlyph shape={s} /> }))}
              onChange={(shape) => set({ eyes: { shape } })}
            />
          </Row>
          <Slider title={t.editor.fields.size} value={face.eyes.size} onChange={(size) => set({ eyes: { size } })} />
          <Slider title={t.editor.fields.spacing} value={face.eyes.spacing} onChange={(spacing) => set({ eyes: { spacing } })} />
          <Slider title={t.editor.fields.tilt} value={face.eyes.tilt} min={-1} onChange={(tilt) => set({ eyes: { tilt } })} />
          <Colour title={t.editor.fields.sclera} value={face.eyes.sclera} onChange={(sclera) => set({ eyes: { sclera } })} />
          <Slider title={t.editor.fields.shine} value={face.eyes.shine} onChange={(shine) => set({ eyes: { shine } })} />
          <Slider title={t.editor.fields.glow} value={face.eyes.glow} onChange={(glow) => set({ eyes: { glow } })} />
        </div>
      </Section>

      <Section title={t.editor.groups.iris}>
        <div className="group">
          <Colour title={t.editor.fields.irisColor} value={face.eyes.iris.color} onChange={(color) => set({ eyes: { iris: { color } } })} />
          <Slider title={t.editor.fields.irisSize} value={face.eyes.iris.size} onChange={(size) => set({ eyes: { iris: { size } } })} />
          <Row title={t.editor.fields.pupilShape}>
            <Glyphs
              label={t.editor.fields.pupilShape}
              value={face.eyes.pupil.shape}
              options={PUPIL_SHAPES.map((s) => ({ value: s, title: t.pupils[s], glyph: <PupilGlyph shape={s} /> }))}
              onChange={(shape) => set({ eyes: { pupil: { shape } } })}
            />
          </Row>
          <Colour title={t.editor.fields.pupilColor} value={face.eyes.pupil.color} onChange={(color) => set({ eyes: { pupil: { color } } })} />
          <Slider title={t.editor.fields.pupilSize} value={face.eyes.pupil.size} onChange={(size) => set({ eyes: { pupil: { size } } })} />
        </div>
      </Section>

      <Section title={t.editor.groups.lids}>
        <div className="group">
          <Slider title={t.editor.fields.lidRest} value={face.eyes.lids.rest} onChange={(rest) => set({ eyes: { lids: { rest } } })} />
          <Colour title={t.editor.fields.lidColor} value={face.eyes.lids.color} onChange={(color) => set({ eyes: { lids: { color } } })} />
          <Choice title={t.editor.fields.lashes} value={face.eyes.lashes} options={LASHES} labels={t.lashes} onChange={(lashes) => set({ eyes: { lashes } })} />
        </div>
      </Section>

      <Section title={t.editor.groups.brows}>
        <div className="group">
          <Choice title={t.editor.fields.style} value={face.brows.style} options={BROW_STYLES} labels={t.brows} onChange={(style) => set({ brows: { style } })} />
          <Colour title={t.editor.fields.color} value={face.brows.color} onChange={(color) => set({ brows: { color } })} />
        </div>
      </Section>

      <Section title={t.editor.groups.mouth}>
        <div className="group">
          <Choice title={t.editor.fields.style} value={face.mouth.style} options={MOUTH_STYLES} labels={t.mouths} onChange={(style) => set({ mouth: { style } })} />
          <Colour title={t.editor.fields.color} value={face.mouth.color} onChange={(color) => set({ mouth: { color } })} />
        </div>
      </Section>

      <Section title={t.editor.groups.cheeks}>
        <div className="group">
          <Slider title={t.editor.fields.blush} value={face.cheeks.blush} onChange={(blush) => set({ cheeks: { blush } })} />
          <Colour title={t.editor.fields.color} value={face.cheeks.color} onChange={(color) => set({ cheeks: { color } })} />
        </div>
      </Section>

      <Section title={t.editor.groups.accessory}>
        <div className="group">
          <Choice title={t.editor.fields.accessory} value={face.accessory} options={ACCESSORIES} labels={t.accessories} onChange={(accessory) => set({ accessory })} />
          <Colour title={t.editor.fields.color} value={face.accessory_color} onChange={(accessory_color) => set({ accessory_color })} />
        </div>
      </Section>

      <Section title={t.editor.groups.color}>
        <div className="group">
          <Colour title={t.editor.fields.accent} desc={t.editor.fields.accentDesc} value={face.palette.accent} onChange={(accent) => set({ palette: { accent } })} />
        </div>
      </Section>

      <Section title={t.editor.groups.motion}>
        <div className="group">
          <Slider title={t.editor.fields.blink} value={face.motion.blink} onChange={(blink) => set({ motion: { blink } })} />
          <Slider title={t.editor.fields.curiosity} desc={t.editor.fields.curiosityDesc} value={face.motion.curiosity} onChange={(curiosity) => set({ motion: { curiosity } })} />
          <Slider title={t.editor.fields.liveliness} desc={t.editor.fields.livelinessDesc} value={face.motion.liveliness} onChange={(liveliness) => set({ motion: { liveliness } })} />
          <Slider
            title={t.editor.fields.sleepy}
            value={face.motion.sleepy_after_min}
            min={0}
            max={60}
            step={1}
            format={(n) => (n === 0 ? t.editor.fields.sleepyNever : t.editor.fields.minutes(n))}
            onChange={(sleepy_after_min) => set({ motion: { sleepy_after_min: Math.min(MAX_SLEEPY_MIN, sleepy_after_min) } })}
          />
          <Row title={t.editor.fields.emotes} desc={t.editor.fields.emotesDesc}>
            <span />
          </Row>
          <div className="fe-emotes">
            {IDLE_EMOTES.map((e) => {
              const on = face.idle_emotes.includes(e);
              return (
                <button
                  key={e}
                  type="button"
                  className="chip"
                  aria-pressed={on}
                  onClick={() => {
                    const next = on ? face.idle_emotes.filter((x) => x !== e) : [...face.idle_emotes, e];
                    props.onChange(clampFace({ ...face, idle_emotes: next }, face));
                    if (!on) play(e);
                  }}
                >
                  {t.emotes[e]}
                </button>
              );
            })}
          </div>
        </div>
      </Section>
    </div>
  );
}

function Preview(props: { face: FaceConfig; state: FaceState; level: number; emote: { kind: IdleEmote; nonce: number } | null; pointer: PointerPos | null; t: FaceStrings }) {
  const small = useRef<HTMLDivElement>(null);
  const big = useRef<HTMLDivElement>(null);
  const smallGaze = useGazeToward(small, props.pointer);
  const bigGaze = useGazeToward(big, props.pointer);
  return (
    <div className="fe-stage">
      <div className="fe-notch" ref={small} aria-label={props.t.editor.collapsed}>
        <Face config={props.face} state={props.state} gaze={smallGaze} level={props.level} size={32} emote={props.emote} autoSleep={false} />
      </div>
      <div className="fe-open" ref={big} aria-label={props.t.editor.expanded}>
        <Face config={props.face} state={props.state} gaze={bigGaze} level={props.level} size={128} emote={props.emote} autoSleep={false} />
      </div>
    </div>
  );
}

function Slider(props: { title: string; desc?: ReactNode; value: number; min?: number; max?: number; step?: number; format?: (n: number) => string; onChange: (n: number) => void }) {
  const { min = 0, max = 1, step = 0.01 } = props;
  const shown = props.format ? props.format(props.value) : `${Math.round(props.value * 100)}`;
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

function Colour(props: { title: string; desc?: ReactNode; value: string; onChange: (hex: string) => void }) {
  return (
    <Row title={props.title} desc={props.desc}>
      <label className="fe-colour">
        <span className="fe-hex">{props.value.toUpperCase()}</span>
        <input type="color" value={props.value.toLowerCase()} aria-label={props.title} onChange={(e) => props.onChange(e.target.value.toUpperCase())} />
      </label>
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
        <button key={o.value} type="button" role="radio" className="swatch" aria-checked={o.value === props.value} title={o.title} aria-label={o.title} onClick={() => props.onChange(o.value)}>
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
