// "Наряди Дружка": the face lab's dressing room for the Buddy face. Buddy
// in C2 at both island sizes following the mouse, quick pickers for
// everything FaceConfig lets the owner and the synth change on pill eyes,
// every accessory live at once, and a button that copies the look as a
// FaceConfig.
//
// The same file has the review matrices the screenshots are taken from
// (FaceLab picks them by URL):
//   ?dress=shots              every accessory: left, centre, right, up, blink, done, sleeping, at 84 and 34 px
//   ?dress=shots&rows=colours the same for a strip of eye colours (also rows=mouths, brows, lashes)
//   ?dress=shots&cols=states  every accessory in the states with their own look and symbols
//   ?dress=sweep              every accessory with a gaze the page script sets (window.__dressGaze(x, y)),
//                             for slow-motion frame strips (add slow=1)
// Each takes the pickers' params too: acc, sclera, acccolor, mouth, brows,
// blush, cheek, lashes, shine, glow (e.g. ?dress=shots&brows=arched&blush=0.6).

import { useEffect, useMemo, useRef, useState, type ReactElement } from "react";
import { Face } from "./Face";
import { faceStrings } from "./i18n";
import { copyText } from "./labCopy";
import { setFaceTimeScale, type Vec } from "./motion";
import type { FaceAttention, SmoothTuning } from "./rig";
import {
  ACCESSORIES,
  BROW_STYLES,
  LASHES,
  MOUTH_STYLES,
  clampFace,
  deepMerge,
  presetFace,
  type Accessory,
  type FaceConfig,
  type FaceState,
  type IdleEmote,
} from "./schema";
import { useGazeToward, type PointerPos } from "./useGaze";

const params = new URLSearchParams(window.location.search);
const REACH = Number(params.get("reach")) > 0 ? Number(params.get("reach")) : 600;
const ru = faceStrings("ru");

/** Eye colours to try: light, saturated, dark and near-black. */
const EYE_COLOURS: { hex: string; name: string }[] = [
  { hex: "#FAFAFA", name: "белый" },
  { hex: "#FFF1D6", name: "сливочный" },
  { hex: "#FFB3CF", name: "розовый" },
  { hex: "#C9B6FF", name: "лавандовый" },
  { hex: "#9FD4FF", name: "небесный" },
  { hex: "#A8F0D4", name: "мятный" },
  { hex: "#FFE066", name: "жёлтый" },
  { hex: "#FF9F43", name: "оранжевый" },
  { hex: "#E5484D", name: "красный" },
  { hex: "#2BB673", name: "изумрудный" },
  { hex: "#2F6BFF", name: "синий" },
  { hex: "#1B2A6B", name: "тёмно-синий" },
  { hex: "#6B1020", name: "бордовый" },
  { hex: "#141418", name: "почти чёрный" },
];

const ACC_COLOURS = ["#FAFAFA", "#FFD166", "#FF6FA8", "#7CC8FF", "#9B7BFF", "#5BE3A0", "#FF5A4E", "#2A2A33"];
const CHEEK_COLOURS = ["#FF8FA3", "#FF6F91", "#FFB38A", "#C9A7FF"];

const STATES: { id: FaceState; label: string }[] = [
  { id: "idle", label: ru.states.idle },
  { id: "listening", label: ru.states.listening },
  { id: "thinking", label: ru.states.thinking },
  { id: "working", label: ru.states.working },
  { id: "approval", label: ru.states.approval },
  { id: "done", label: ru.states.done },
  { id: "error", label: ru.states.error },
  { id: "sleeping", label: ru.states.sleeping },
];

const EMOTES: { id: IdleEmote; label: string }[] = [
  { id: "wink", label: "подмигнуть" },
  { id: "squint", label: "прищуриться" },
  { id: "surprised", label: "удивиться" },
  { id: "yawn", label: "зевнуть" },
  { id: "heart_eyes", label: "влюбиться" },
  { id: "look_around", label: "оглядеться" },
];

/** Buddy, with whatever the URL asks for on top (the review matrices). */
export function dressFromParams(): FaceConfig {
  const patch: Record<string, unknown> = {};
  const num = (k: string) => (params.get(k) !== null ? Number(params.get(k)) : undefined);
  const hex = (k: string) => {
    const v = params.get(k);
    return v ? `#${v.replace(/^#/, "")}` : undefined;
  };
  if (params.get("acc")) patch.accessory = params.get("acc");
  if (hex("acccolor")) patch.accessory_color = hex("acccolor");
  const eyes: Record<string, unknown> = {};
  if (hex("sclera")) eyes.sclera = hex("sclera");
  if (num("shine") !== undefined) eyes.shine = num("shine");
  if (num("glow") !== undefined) eyes.glow = num("glow");
  if (params.get("lashes")) eyes.lashes = params.get("lashes");
  if (Object.keys(eyes).length) patch.eyes = eyes;
  if (params.get("mouth")) patch.mouth = { style: params.get("mouth") };
  if (params.get("brows")) patch.brows = { style: params.get("brows") };
  const cheeks: Record<string, unknown> = {};
  if (num("blush") !== undefined) cheeks.blush = num("blush");
  if (hex("cheek")) cheeks.color = hex("cheek");
  if (Object.keys(cheeks).length) patch.cheeks = cheeks;
  const base = presetFace("buddy");
  return clampFace(deepMerge(base, patch), base);
}

// ---------- the faces ----------

interface SlotProps {
  config: FaceConfig;
  size: number;
  state: FaceState;
  pointer: PointerPos | null;
  /** A fixed gaze instead of the mouse. */
  fixed?: Vec | null;
  still?: boolean;
  blink?: number;
  emote?: { kind: IdleEmote; nonce: number } | null;
  /** What the host says about the pointer (the matrices: is it "near"). */
  attention?: FaceAttention;
  rigTuning?: Partial<SmoothTuning>;
}

function Slot(p: SlotProps) {
  const ref = useRef<HTMLDivElement>(null);
  const gaze = useGazeToward(ref, p.pointer, REACH, p.fixed ?? null);
  return (
    <div ref={ref} className="dl-slot">
      <Face
        config={p.config}
        state={p.state}
        gaze={p.fixed ?? gaze}
        size={p.size}
        motionStyle="smooth"
        autoSleep={false}
        reducedMotion={false}
        still={p.still}
        blink={p.blink}
        emote={p.emote}
        attention={p.attention}
        rigTuning={p.rigTuning}
      />
    </div>
  );
}

/** The open island (84 px) as a black card. */
function OpenIsland(p: Omit<SlotProps, "size">) {
  return (
    <div className="dl-open" data-size="84">
      <Slot {...p} size={84} />
    </div>
  );
}

/** The closed island as it really sits: a 32 px band flush with the top
 * of the screen, rounded at the bottom, the 34 px face in its left ear 8 px
 * in and 1 px above the edge (IslandApp), clipped by the band. */
function ClosedIsland(p: Omit<SlotProps, "size">) {
  return (
    <div className="dl-screen">
      <div className="dl-closed" data-size="34">
        <div className="dl-closed-face">
          <Slot {...p} size={34} />
        </div>
      </div>
    </div>
  );
}

// ---------- pickers ----------

function Chips<T extends string>(p: { label: string; value: T; options: readonly T[]; names: Record<T, string>; onChange: (v: T) => void; name: string }) {
  return (
    <div className="dl-row">
      <span className="dl-label">{p.label}</span>
      <div className="dl-chips" role="radiogroup" aria-label={p.label}>
        {p.options.map((o) => (
          <button key={o} role="radio" aria-checked={p.value === o} className={p.value === o ? "on" : ""} onClick={() => p.onChange(o)} data-pick={`${p.name}:${o}`}>
            {p.names[o]}
          </button>
        ))}
      </div>
    </div>
  );
}

function Swatches(p: { label: string; value: string; options: { hex: string; name: string }[]; onChange: (v: string) => void; name: string }) {
  const v = p.value.toUpperCase();
  return (
    <div className="dl-row">
      <span className="dl-label">{p.label}</span>
      <div className="dl-swatches" role="radiogroup" aria-label={p.label}>
        {p.options.map((o) => (
          <button
            key={o.hex}
            role="radio"
            aria-checked={v === o.hex}
            aria-label={o.name}
            title={o.name}
            className={`dl-swatch${v === o.hex ? " on" : ""}`}
            style={{ background: o.hex }}
            onClick={() => p.onChange(o.hex)}
            data-pick={`${p.name}:${o.hex}`}
          />
        ))}
        <label className="dl-free" title="свой цвет">
          <input type="color" value={p.value.toLowerCase()} onChange={(e) => p.onChange(e.currentTarget.value.toUpperCase())} aria-label={`${p.label}: свой цвет`} />
          <span>{v}</span>
        </label>
      </div>
    </div>
  );
}

function Range(p: { label: string; value: number; onChange: (v: number) => void; name: string }) {
  return (
    <div className="dl-row">
      <span className="dl-label">{p.label}</span>
      <div className="dl-range">
        <input type="range" name={p.name} min={0} max={1} step={0.05} value={p.value} onChange={(e) => p.onChange(Number(e.currentTarget.value))} />
        <output>{Math.round(p.value * 100)}%</output>
      </div>
    </div>
  );
}

// ---------- the section ----------

type Patch = Record<string, unknown>;

export function DressLab(props: { pointer: PointerPos | null }): ReactElement {
  const [face, setFace] = useState<FaceConfig>(dressFromParams);
  const [state, setState] = useState<FaceState>("idle");
  const [emote, setEmote] = useState<{ kind: IdleEmote; nonce: number } | null>(null);
  const [copied, setCopied] = useState<"" | "ok" | "manual">("");
  const set = (patch: Patch) => {
    setFace((f) => clampFace(deepMerge(f, patch), f));
    setCopied("");
  };
  const json = useMemo(() => JSON.stringify(clampFace(face), null, 2), [face]);
  const copy = async () => setCopied((await copyText(json)) ? "ok" : "manual");
  const pointer = props.pointer;
  const onState = (s: FaceState) => {
    setState(s);
    setEmote(null);
  };

  return (
    <section className="dl" aria-labelledby="dl-title">
      <header className="dl-head">
        <h2 id="dl-title">Наряди Дружка</h2>
        <p>C2, как в приложении. Слева открытый остров (84 px) и закрытый у края экрана (34 px), справа всё, что можно поменять. Ниже все аксессуары сразу, вживую.</p>
      </header>
      <div className="dl-main">
        <div className="dl-stage">
          <OpenIsland config={face} state={state} pointer={pointer} emote={emote} />
          <ClosedIsland config={face} state={state} pointer={pointer} emote={emote} />
          <div className="dl-chips dl-states" role="radiogroup" aria-label="Состояние">
            {STATES.map((s) => (
              <button key={s.id} role="radio" aria-checked={state === s.id} className={state === s.id ? "on" : ""} onClick={() => onState(s.id)} data-dress-state={s.id}>
                {s.label}
              </button>
            ))}
          </div>
          <div className="dl-chips dl-emotes" aria-label="Привычки (в покое)">
            {EMOTES.map((e) => (
              <button
                key={e.id}
                disabled={state !== "idle"}
                onClick={() => setEmote((cur) => ({ kind: e.id, nonce: (cur?.nonce ?? 0) + 1 }))}
                data-emote={e.id}
              >
                {e.label}
              </button>
            ))}
          </div>
        </div>
        <div className="dl-pickers">
          <Chips label="Аксессуар" name="acc" value={face.accessory} options={ACCESSORIES} names={ru.accessories} onChange={(accessory) => set({ accessory })} />
          <Swatches label="Цвет аксессуара" name="acccolor" value={face.accessory_color} options={ACC_COLOURS.map((hex) => ({ hex, name: hex }))} onChange={(accessory_color) => set({ accessory_color })} />
          <Swatches label="Цвет глаз" name="sclera" value={face.eyes.sclera} options={EYE_COLOURS} onChange={(sclera) => set({ eyes: { sclera } })} />
          <Range label="Блик" name="shine" value={face.eyes.shine} onChange={(shine) => set({ eyes: { shine } })} />
          <Range label="Свечение" name="glow" value={face.eyes.glow} onChange={(glow) => set({ eyes: { glow } })} />
          <Chips label="Рот" name="mouth" value={face.mouth.style} options={MOUTH_STYLES} names={ru.mouths} onChange={(style) => set({ mouth: { style } })} />
          <Chips label="Брови" name="brows" value={face.brows.style} options={BROW_STYLES} names={ru.brows} onChange={(style) => set({ brows: { style } })} />
          <Chips label="Ресницы" name="lashes" value={face.eyes.lashes} options={LASHES} names={ru.lashes} onChange={(lashes) => set({ eyes: { lashes } })} />
          <Range label="Румянец" name="blush" value={face.cheeks.blush} onChange={(blush) => set({ cheeks: { blush } })} />
          <Swatches label="Цвет румянца" name="cheek" value={face.cheeks.color} options={CHEEK_COLOURS.map((hex) => ({ hex, name: hex }))} onChange={(color) => set({ cheeks: { color } })} />
          <div className="dl-actions">
            <button className="dl-copy" onClick={copy} data-action="copy-face">
              скопировать лицо
            </button>
            <button
              onClick={() => {
                setFace(presetFace("buddy"));
                setCopied("");
              }}
              data-action="reset-face"
            >
              вернуть Дружка
            </button>
            <span className="dl-copied" role="status">
              {copied === "ok" ? "скопировано (FaceConfig)" : copied === "manual" ? "буфер обмена недоступен, скопируйте из поля ниже" : ""}
            </span>
          </div>
          {/* Shown after every copy: some browsers report a copy over plain
              http that never reached the clipboard. */}
          {copied ? <textarea className="lab-json dl-json" readOnly value={json} onFocus={(e) => e.currentTarget.select()} autoFocus={copied === "manual"} /> : null}
        </div>
      </div>
      <h3 className="dl-grid-title">Все аксессуары</h3>
      <div className="dl-grid">
        {ACCESSORIES.map((a) => (
          <AccCard key={a} acc={a} face={face} state={state} pointer={pointer} active={a === face.accessory} onPick={() => set({ accessory: a })} />
        ))}
      </div>
    </section>
  );
}

function AccCard(p: { acc: Accessory; face: FaceConfig; state: FaceState; pointer: PointerPos | null; active: boolean; onPick: () => void }) {
  const config = useMemo(() => ({ ...p.face, accessory: p.acc }), [p.face, p.acc]);
  return (
    <figure className={`dl-card${p.active ? " on" : ""}`} data-acc={p.acc}>
      <OpenIsland config={config} state={p.state} pointer={p.pointer} />
      <ClosedIsland config={config} state={p.state} pointer={p.pointer} />
      <figcaption>
        <button onClick={p.onPick} aria-pressed={p.active}>
          {ru.accessories[p.acc]}
        </button>
      </figcaption>
    </figure>
  );
}

// ---------- review matrices (screenshots) ----------

interface Col {
  key: string;
  label: string;
  gaze?: Vec;
  state?: FaceState;
  still?: boolean;
  blink?: number;
  patch?: Patch;
  /** The pointer is at the face: C2's happy squint. */
  near?: boolean;
}

const SHOT_COLS: Col[] = [
  { key: "left", label: "влево", gaze: { x: -0.9, y: 0.05 } },
  { key: "centre", label: "прямо", gaze: { x: 0, y: 0.05 } },
  { key: "right", label: "вправо", gaze: { x: 0.9, y: 0.05 } },
  { key: "up", label: "вверх", gaze: { x: 0.35, y: -0.95 } },
  { key: "squint", label: "прищур", gaze: { x: 0.05, y: 0.12 }, near: true },
  { key: "blink", label: "моргает", gaze: { x: 0, y: 0.05 }, still: true, blink: 1 },
  { key: "done", label: "готово", gaze: { x: 0, y: 0 }, state: "done" },
  { key: "sleep", label: "спит", gaze: { x: 0, y: 0 }, state: "sleeping" },
];

const COLOUR_COLS: Col[] = [
  { key: "left", label: "влево", gaze: { x: -0.9, y: 0.05 } },
  { key: "centre", label: "прямо", gaze: { x: 0, y: 0.05 } },
  { key: "right", label: "вправо", gaze: { x: 0.9, y: -0.2 } },
  { key: "done", label: "готово", gaze: { x: 0, y: 0 }, state: "done" },
  { key: "blink", label: "моргает", gaze: { x: 0, y: 0.05 }, still: true, blink: 1 },
  { key: "shine", label: "блик 1", gaze: { x: 0.3, y: 0 }, patch: { eyes: { shine: 1 } } },
  { key: "glow", label: "свечение 0.6", gaze: { x: 0.3, y: 0 }, patch: { eyes: { glow: 0.6 } } },
];

/** The states with their own look and the symbols beside the right eye. */
const STATE_COLS: Col[] = [
  { key: "thinking", label: "думает", state: "thinking" },
  { key: "working", label: "работает", state: "working" },
  { key: "approval", label: "вопрос", gaze: { x: 0, y: 0.3 }, state: "approval" },
  { key: "error", label: "ошибка", state: "error" },
  { key: "listening", label: "слушает", state: "listening" },
  { key: "driving", label: "за компьютером", gaze: { x: 0.6, y: 0.4 }, state: "driving" },
  { key: "accepted", label: "принято", state: "accepted" },
];

declare global {
  interface Window {
    __dressGaze?: (x: number, y: number) => void;
  }
}

/** The matrices hold still for the camera: no boredom glances, no random
 * blinks or emotes in the few seconds before the shot. */
const STEADY: Partial<SmoothTuning> = { bored: false };

export function DressShots(props: { mode: string }): ReactElement {
  const base = useMemo(() => {
    const f = dressFromParams();
    return { ...f, motion: { ...f.motion, blink: 0 }, idle_emotes: [] };
  }, []);
  const sweep = props.mode === "sweep";
  const rowsKind = params.get("rows");
  const colours = rowsKind === "colours";
  const [gaze, setGaze] = useState<Vec>({ x: -0.9, y: 0.05 });
  useEffect(() => {
    setFaceTimeScale(params.get("slow") === "1" ? 0.25 : 1);
    window.__dressGaze = (x, y) => setGaze({ x, y });
    return () => {
      delete window.__dressGaze;
    };
  }, []);
  const with_ = (patch: Patch) => clampFace(deepMerge(base, patch), base);
  const rows: { key: string; label: string; config: FaceConfig }[] = colours
    ? EYE_COLOURS.map((c) => ({ key: c.hex.slice(1), label: c.name, config: with_({ eyes: { sclera: c.hex } }) }))
    : rowsKind === "mouths"
      ? MOUTH_STYLES.map((m) => ({ key: `mouth_${m}`, label: ru.mouths[m], config: with_({ mouth: { style: m } }) }))
      : rowsKind === "brows"
        ? BROW_STYLES.map((b) => ({ key: `brows_${b}`, label: ru.brows[b], config: with_({ brows: { style: b } }) }))
        : rowsKind === "lashes"
          ? LASHES.map((l) => ({ key: `lashes_${l}`, label: ru.lashes[l], config: with_({ eyes: { lashes: l } }) }))
          : ACCESSORIES.map((a) => ({ key: a, label: ru.accessories[a], config: { ...base, accessory: a } }));
  const cols: Col[] = sweep ? [{ key: "live", label: "", gaze }] : colours ? COLOUR_COLS : params.get("cols") === "states" ? STATE_COLS : SHOT_COLS;
  return (
    <main className="dl-shots" data-mode={props.mode} style={{ ["--cols" as string]: cols.length }}>
      <div className="dl-shots-head">
        <span />
        {[84, 34].map((size) => cols.map((c) => <span key={`${size}${c.key}`}>{sweep ? `${size} px` : `${c.label} ${size}`}</span>))}
      </div>
      {rows.map((r) => (
        <div key={r.key} className="dl-shots-row" data-row={r.key}>
          <span className="dl-shots-label">{r.label}</span>
          {[84, 34].map((size) =>
            cols.map((c) => {
              const config = c.patch ? clampFace(deepMerge(r.config, c.patch), r.config) : r.config;
              const p = { config, state: c.state ?? "idle", pointer: null, fixed: c.gaze, still: c.still, blink: c.blink, attention: { near: Boolean(c.near) }, rigTuning: STEADY };
              return <div key={`${size}${c.key}`} className="dl-shots-cell" data-col={`${c.key}_${size}`}>{size === 84 ? <OpenIsland {...p} /> : <ClosedIsland {...p} />}</div>;
            }),
          )}
        </div>
      ))}
    </main>
  );
}

