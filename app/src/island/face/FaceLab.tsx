// The face lab: the Buddy face in each motion style side by side, so the
// owner can pick one (or a mix) before it goes into the island. Browser
// only. Open http://127.0.0.1:1420/src/island/face/lab.html while
// `npm run dev` runs, or build the single-file copy with
// `node scripts/build-face-lab.mjs` (writes /tmp/face-lab/index.html).
//
// Every face tracks the real mouse anywhere on the page. Each column shows
// the open island (84 px) and under it the closed one (34 px). C2 (the
// smooth style) has sliders for live tuning and a button that copies the
// values as JSON.
//
// URL parameters (screenshots and checks): state=idle|listening|thinking|done,
// slow=1 (0.25x clock), preset=<name> (default buddy), reach=<px> (default 600),
// only=<tags> (columns to show, e.g. only=0ABCC2 or only=all; default 0,C,C2).
// Keys: 1-4 states, S slow motion, R reset.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactElement } from "react";
import { Face } from "./Face";
import { setFaceTimeScale, type Vec } from "./motion";
import { SMOOTH, SMOOTH_DEFAULTS, smoothDerived, type MotionStyle, type SmoothTuning } from "./rig";
import { presetFace, type FaceState } from "./schema";
import { useFakeVoice, useGazeToward, useWindowPointer, type PointerPos } from "./useGaze";

const params = new URLSearchParams(window.location.search);

/** The island's own reach (src-tauri island/mod.rs GAZE_REACH): the gaze
 * vector reaches length 1 this many px from the face. ?reach= overrides it
 * (screenshots use a short one so a nearby pointer means a full look). */
const REACH = Number(params.get("reach")) > 0 ? Number(params.get("reach")) : 600;

const STATES: { id: FaceState; label: string; key: string }[] = [
  { id: "idle", label: "покой", key: "1" },
  { id: "listening", label: "слушает", key: "2" },
  { id: "thinking", label: "думает", key: "3" },
  { id: "done", label: "готово", key: "4" },
];

interface Variant {
  style: MotionStyle;
  tag: string;
  title: string;
  hint: string;
}

const VARIANTS: Variant[] = [
  { style: "classic", tag: "0", title: "0 (было в 0.1.16)", hint: "старое: глаза скользят за курсором и наклоняются" },
  { style: "cozmo", tag: "A", title: "A: перспектива + саккады", hint: "поворот головы вместо скольжения, глаза прыгают и замирают" },
  { style: "gloss", tag: "B", title: "B: + объём и блик", hint: "то же, плюс объём: градиент, тёмный край, блик от света слева сверху" },
  { style: "head", tag: "C", title: "C (было)", hint: "первая версия: голова пружинит, взгляд прыгает, дыхание рывками" },
  { style: "smooth", tag: "C2", title: "C2: плавный (в приложении)", hint: "тот же характер, но плавно: взгляд скользит, прыгает только на большие сдвиги, голова без раскачки" },
];

/** ?only=: tags to show (0, A, B, C, C2), or "all". Default: 0, C, C2. */
const shown = (() => {
  const raw = params.get("only");
  if (raw === "all") return VARIANTS;
  const tags = raw ? (raw.toUpperCase().match(/C2|[0ABC]/g) ?? []) : ["0", "C", "C2"];
  return VARIANTS.filter((v) => tags.includes(v.tag));
})();

const initialState = (STATES.find((s) => s.id === params.get("state"))?.id ?? "idle") as FaceState;
const initialSlow = params.get("slow") === "1";
const face = presetFace(params.get("preset") ?? "buddy");

interface SlotProps {
  style: MotionStyle;
  size: number;
  state: FaceState;
  level: number;
  pointer: PointerPos | null;
  className: string;
  tuning?: Partial<SmoothTuning>;
}

function FaceSlot(props: SlotProps) {
  const ref = useRef<HTMLDivElement>(null);
  const gaze: Vec | null = useGazeToward(ref, props.pointer, REACH);
  return (
    <div ref={ref} className={props.className}>
      {/* Motion always on: the lab exists to look at it, even with the system's reduce-motion setting. */}
      <Face
        config={face}
        state={props.state}
        gaze={gaze}
        level={props.level}
        size={props.size}
        motionStyle={props.style}
        rigTuning={props.tuning}
        autoSleep={false}
        reducedMotion={false}
      />
    </div>
  );
}

function Column(props: { v: Variant; state: FaceState; level: number; pointer: PointerPos | null; tuning?: SmoothTuning; children?: ReactElement }) {
  const { v } = props;
  return (
    <section className={`lab-col lab-${v.tag}`} data-variant={v.tag}>
      <header>
        <h2>{v.title}</h2>
        <p>{v.hint}</p>
      </header>
      <div className="lab-open" data-size="84">
        <FaceSlot className="lab-face" style={v.style} size={84} state={props.state} level={props.level} pointer={props.pointer} tuning={props.tuning} />
      </div>
      <div className="lab-closed" data-size="34">
        <FaceSlot className="lab-face-small" style={v.style} size={34} state={props.state} level={props.level} pointer={props.pointer} tuning={props.tuning} />
      </div>
      {props.children}
    </section>
  );
}

interface SliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  show: (v: number) => string;
  onChange: (v: number) => void;
  name: string;
  disabled?: boolean;
}

function Slider(p: SliderProps) {
  return (
    <label className={`lab-slider${p.disabled ? " off" : ""}`}>
      <span className="lab-slider-top">
        <span>{p.label}</span>
        <output>{p.show(p.value)}</output>
      </span>
      <input
        type="range"
        name={p.name}
        min={p.min}
        max={p.max}
        step={p.step}
        value={p.value}
        disabled={p.disabled}
        onChange={(e) => p.onChange(Number(e.currentTarget.value))}
      />
    </label>
  );
}

/** Copies text: the async clipboard where allowed, else the old
 * select-and-copy (plain http on the LAN is not a secure context). */
async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // Fall through to the old way.
  }
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.setAttribute("readonly", "");
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  let ok = false;
  try {
    ok = document.execCommand("copy");
  } catch {
    ok = false;
  }
  ta.remove();
  return ok;
}

function TuningPanel(props: { tuning: SmoothTuning; onChange: (t: SmoothTuning) => void }) {
  const t = props.tuning;
  const set = (patch: Partial<SmoothTuning>) => props.onChange({ ...t, ...patch });
  const d = smoothDerived(t);
  const [copied, setCopied] = useState<"" | "ok" | "manual">("");
  const json = useMemo(
    () =>
      JSON.stringify(
        {
          style: "smooth",
          tuning: t,
          derived: {
            saccade_from: +d.saccadeAt.toFixed(3),
            pursuit_w: +d.pursuitW.toFixed(2),
            saccade_ms: [Math.round(d.durShort * 1000), Math.round(d.durLong * 1000)],
            pitch_deg: +(t.turn * SMOOTH.pitchOfTurn).toFixed(1),
            breath_scale: +(SMOOTH.breathScale * t.breath).toFixed(4),
          },
        },
        null,
        2,
      ),
    [t, d],
  );
  const copy = async () => setCopied((await copyText(json)) ? "ok" : "manual");
  return (
    <div className="lab-tune" aria-label="Настройки C2">
      <Slider
        name="smoothness"
        label="плавность взгляда"
        value={t.smoothness}
        min={0}
        max={1}
        step={0.05}
        show={() => `прыжок от ${d.saccadeAt.toFixed(2)}, ${Math.round(d.durShort * 1000)}-${Math.round(d.durLong * 1000)} мс`}
        onChange={(v) => set({ smoothness: v })}
      />
      <Slider name="headK" label="инерция головы: жёсткость" value={t.headK} min={8} max={120} step={1} show={(v) => `${v}`} onChange={(v) => set({ headK: v })} />
      <Slider
        name="headZeta"
        label="инерция головы: затухание"
        value={t.headZeta}
        min={0.4}
        max={1.1}
        step={0.01}
        show={(v) => (v >= 1 ? `${v.toFixed(2)} (без отскока)` : v.toFixed(2))}
        onChange={(v) => set({ headZeta: v })}
      />
      <Slider name="turn" label="сила поворота головы" value={t.turn} min={0} max={30} step={1} show={(v) => `${v}°`} onChange={(v) => set({ turn: v })} />
      <Slider name="breath" label="дыхание" value={t.breath} min={0} max={2} step={0.05} show={(v) => (v === 0 ? "нет" : `${v.toFixed(2)}×`)} onChange={(v) => set({ breath: v })} />
      <div className="lab-bored">
        <label className="lab-check">
          <input type="checkbox" name="bored" checked={t.bored} onChange={(e) => set({ bored: e.currentTarget.checked })} />
          скука
        </label>
        <Slider
          name="boredAfter"
          label="через"
          value={t.boredAfter}
          min={1}
          max={15}
          step={0.5}
          show={(v) => `${v} с`}
          onChange={(v) => set({ boredAfter: v })}
          disabled={!t.bored}
        />
      </div>
      <div className="lab-tune-actions">
        <button className="lab-copy" onClick={copy} data-action="copy">
          скопировать настройки
        </button>
        <button className="lab-defaults" onClick={() => props.onChange({ ...SMOOTH_DEFAULTS })} data-action="defaults">
          по умолчанию
        </button>
        <span className="lab-copied" role="status">
          {copied === "ok" ? "скопировано" : copied === "manual" ? "не вышло, скопируйте ниже" : ""}
        </span>
      </div>
      {copied === "manual" ? <textarea className="lab-json" readOnly value={json} onFocus={(e) => e.currentTarget.select()} /> : null}
    </div>
  );
}

export function FaceLab(): ReactElement {
  const pointer = useWindowPointer();
  const [state, setState] = useState<FaceState>(initialState);
  const [slow, setSlow] = useState(initialSlow);
  const [epoch, setEpoch] = useState(0);
  const [tuning, setTuning] = useState<SmoothTuning>({ ...SMOOTH_DEFAULTS });
  const level = useFakeVoice(state === "listening");

  useEffect(() => {
    setFaceTimeScale(slow ? 0.25 : 1);
  }, [slow]);

  const reset = useCallback(() => {
    setState("idle");
    setSlow(false);
    setEpoch((e) => e + 1);
  }, []);

  useEffect(() => {
    const on = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      // Arrow keys and digits belong to a focused slider.
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) return;
      const st = STATES.find((s) => s.key === e.key);
      if (st) setState(st.id);
      else if (e.key === "s" || e.key === "S" || e.key === "ы" || e.key === "Ы") setSlow((v) => !v);
      else if (e.key === "r" || e.key === "R" || e.key === "к" || e.key === "К") reset();
    };
    window.addEventListener("keydown", on);
    return () => window.removeEventListener("keydown", on);
  }, [reset]);

  return (
    <main className="lab">
      <div className="lab-top">
        <div>
          <h1>Лаборатория лица: Дружок</h1>
          <p className="lab-sub">Водите мышью по странице, все лица смотрят на курсор. Под C2 ползунки для настройки вживую.</p>
        </div>
        <div className="lab-controls" role="toolbar">
          <div className="lab-seg" role="radiogroup" aria-label="Состояние">
            {STATES.map((s) => (
              <button key={s.id} role="radio" aria-checked={state === s.id} className={state === s.id ? "on" : ""} onClick={() => setState(s.id)} data-state={s.id}>
                {s.label}
                <kbd>{s.key}</kbd>
              </button>
            ))}
          </div>
          <button className={`lab-toggle${slow ? " on" : ""}`} aria-pressed={slow} onClick={() => setSlow((v) => !v)} data-action="slow">
            замедление 0.25× <kbd>S</kbd>
          </button>
          <button className="lab-reset" onClick={reset} data-action="reset">
            сброс <kbd>R</kbd>
          </button>
        </div>
      </div>
      <div className="lab-grid" key={epoch} style={{ gridTemplateColumns: `repeat(${Math.max(1, shown.length)}, minmax(0, 1fr))` }}>
        {shown.map((v) =>
          v.style === "smooth" ? (
            <Column key={v.style} v={v} state={state} level={level} pointer={pointer} tuning={tuning}>
              <TuningPanel tuning={tuning} onChange={setTuning} />
            </Column>
          ) : (
            <Column key={v.style} v={v} state={state} level={level} pointer={pointer} />
          ),
        )}
      </div>
      <aside className="lab-notes">
        <h3>Что попробовать в C и C2</h3>
        <ul>
          <li>Медленно ведите мышью: C2 плавно скользит взглядом, C прыгает ступеньками.</li>
          <li>Оставьте курсор в покое на несколько секунд: заскучает, отведёт взгляд и иногда будет возвращаться.</li>
          <li>Резко дёрните мышью: коротко вздрогнет и распахнёт глаза.</li>
          <li>Задержите курсор возле лица: посмотрит вниз на него и довольно прищурится (C2 ждёт 0.4 с, мимоходом не срабатывает).</li>
          <li>Ещё варианты: ?only=all показывает A и B.</li>
        </ul>
      </aside>
    </main>
  );
}
