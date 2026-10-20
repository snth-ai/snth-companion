// The face lab: the Buddy face in each motion style side by side, so the
// owner can pick one (or a mix) before it goes into the island. Browser
// only. Open http://127.0.0.1:1420/src/island/face/lab.html while
// `npm run dev` runs, or build the single-file copy with
// `node scripts/build-face-lab.mjs` (writes /tmp/face-lab/index.html).
//
// Every face tracks the real mouse anywhere on the page. Each column shows
// the open island (84 px) and under it the closed one (34 px).
//
// URL parameters (screenshots and checks): state=idle|listening|thinking|done,
// slow=1 (0.25x clock), preset=<name> (default buddy), reach=<px> (default 600),
// only=0ABC (show just these columns, for per-variant energy checks).
// Keys: 1-4 states, S slow motion, R reset.

import { useCallback, useEffect, useRef, useState, type ReactElement } from "react";
import { Face } from "./Face";
import { setFaceTimeScale, type Vec } from "./motion";
import type { MotionStyle } from "./rig";
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
  { style: "classic", tag: "0", title: "0 (сейчас)", hint: "как в приложении: глаза скользят за курсором и наклоняются" },
  { style: "cozmo", tag: "A", title: "A: перспектива + саккады", hint: "поворот головы вместо скольжения, глаза прыгают и замирают" },
  { style: "gloss", tag: "B", title: "B: + объём и блик", hint: "то же, плюс объём: градиент, тёмный край, блик от света слева сверху" },
  { style: "head", tag: "C", title: "C: + голова и поведение", hint: "глаза ведут, голова догоняет; скучает, вздрагивает, щурится, дышит" },
];

const only = params.get("only");
const shown = only ? VARIANTS.filter((v) => only.includes(v.tag)) : VARIANTS;

const initialState = (STATES.find((s) => s.id === params.get("state"))?.id ?? "idle") as FaceState;
const initialSlow = params.get("slow") === "1";
const face = presetFace(params.get("preset") ?? "buddy");

function FaceSlot(props: { style: MotionStyle; size: number; state: FaceState; level: number; pointer: PointerPos | null; className: string }) {
  const ref = useRef<HTMLDivElement>(null);
  const gaze: Vec | null = useGazeToward(ref, props.pointer, REACH);
  return (
    <div ref={ref} className={props.className}>
      {/* Motion always on: the lab exists to look at it, even with the system's reduce-motion setting. */}
      <Face config={face} state={props.state} gaze={gaze} level={props.level} size={props.size} motionStyle={props.style} autoSleep={false} reducedMotion={false} />
    </div>
  );
}

function Column(props: { v: Variant; state: FaceState; level: number; pointer: PointerPos | null }) {
  const { v } = props;
  return (
    <section className={`lab-col lab-${v.tag}`} data-variant={v.tag}>
      <header>
        <h2>{v.title}</h2>
        <p>{v.hint}</p>
      </header>
      <div className="lab-open" data-size="84">
        <FaceSlot className="lab-face" style={v.style} size={84} state={props.state} level={props.level} pointer={props.pointer} />
      </div>
      <div className="lab-closed" data-size="34">
        <FaceSlot className="lab-face-small" style={v.style} size={34} state={props.state} level={props.level} pointer={props.pointer} />
      </div>
    </section>
  );
}

export function FaceLab(): ReactElement {
  const pointer = useWindowPointer();
  const [state, setState] = useState<FaceState>(initialState);
  const [slow, setSlow] = useState(initialSlow);
  const [epoch, setEpoch] = useState(0);
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
          <p className="lab-sub">Водите мышью по странице, все лица смотрят на курсор. Каждый вариант добавляет к предыдущему.</p>
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
      <div className="lab-grid" key={epoch}>
        {shown.map((v) => (
          <Column key={v.style} v={v} state={state} level={level} pointer={pointer} />
        ))}
      </div>
      <aside className="lab-notes">
        <h3>Что попробовать в C</h3>
        <ul>
          <li>Оставьте курсор в покое на 3 секунды: заскучает, отведёт взгляд и иногда будет возвращаться.</li>
          <li>Резко дёрните мышью: коротко вздрогнет и распахнёт глаза.</li>
          <li>Поднесите курсор вплотную к лицу: посмотрит вниз на него и довольно прищурится.</li>
          <li>Замедление показывает саккады: быстрый прыжок, лёгкий перелёт и возврат.</li>
        </ul>
      </aside>
    </main>
  );
}
