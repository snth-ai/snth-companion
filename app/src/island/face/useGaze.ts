// Gaze from the mouse inside a window (editor and demo; the island itself
// gets its gaze from the app, which reads the system cursor).

import { useEffect, useLayoutEffect, useState, type RefObject } from "react";
import type { Vec } from "./motion";

export interface PointerPos {
  x: number;
  y: number;
}

/** The pointer in client coordinates, at most once per frame. */
export function useWindowPointer(): PointerPos | null {
  const [p, setP] = useState<PointerPos | null>(null);
  useEffect(() => {
    let raf = 0;
    let last: PointerPos | null = null;
    const on = (e: PointerEvent) => {
      last = { x: e.clientX, y: e.clientY };
      if (!raf)
        raf = requestAnimationFrame(() => {
          raf = 0;
          setP(last);
        });
    };
    window.addEventListener("pointermove", on);
    return () => {
      window.removeEventListener("pointermove", on);
      cancelAnimationFrame(raf);
    };
  }, []);
  return p;
}

/** The pointer as seen from the centre of an element: a vector that reaches
 * length 1 at `reach` px and never goes past it. */
export function useGazeToward(ref: RefObject<HTMLElement | null>, pointer: PointerPos | null, reach = 360, fixed: Vec | null = null): Vec | null {
  const [gaze, setGaze] = useState<Vec | null>(fixed);
  useLayoutEffect(() => {
    if (fixed || !pointer || !ref.current) return;
    const r = ref.current.getBoundingClientRect();
    const dx = (pointer.x - (r.left + r.width / 2)) / reach;
    const dy = (pointer.y - (r.top + r.height / 2)) / reach;
    const len = Math.hypot(dx, dy);
    const s = len > 1 ? 1 / len : 1;
    setGaze({ x: dx * s, y: dy * s });
  }, [pointer, ref, reach, fixed]);
  return gaze;
}

/** A pretend voice for previews: syllables, pauses, a little noise. */
export function useFakeVoice(on: boolean): number {
  const [level, setLevel] = useState(0);
  useEffect(() => {
    if (!on) {
      setLevel(0);
      return;
    }
    let raf = 0;
    const start = performance.now();
    const tick = (now: number) => {
      const s = (now - start) / 1000;
      const syllable = Math.max(0, Math.sin(s * 9.5)) * (0.5 + 0.5 * Math.sin(s * 1.7 + 1));
      const talking = Math.sin(s * 0.8) > -0.6 ? 1 : 0.05;
      setLevel(Math.min(1, syllable * talking * (0.7 + Math.random() * 0.3)));
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [on]);
  return level;
}
