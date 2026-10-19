// A stand-in for the face module until it is merged: two eyes that look
// where they are told, in the iris colour of the config. Same props as
// the real <Face> and <FaceEditor>, so nothing else changes at the merge.

import type { CSSProperties } from "react";
import type { FaceConfig } from "../backend/types";

export type FaceState = "idle" | "sleeping" | "listening" | "accepted" | "thinking" | "working" | "driving" | "approval" | "done" | "error" | "offline";

function pick(config: FaceConfig | null | undefined, path: string[], fallback: string): string {
  let at: unknown = config;
  for (const key of path) at = at && typeof at === "object" ? (at as Record<string, unknown>)[key] : undefined;
  return typeof at === "string" && /^#[0-9A-Fa-f]{6}$/.test(at) ? at : fallback;
}

export function Face(props: {
  config: FaceConfig | null | undefined;
  state?: FaceState;
  gaze?: { x: number; y: number } | null;
  level?: number;
  size?: number;
  reducedMotion?: boolean;
  compact?: boolean;
  label?: string;
  className?: string;
  style?: CSSProperties;
}) {
  const h = props.size ?? 32;
  const w = Math.round(h * 2.4);
  const iris = pick(props.config, ["eyes", "iris", "color"], "#7A4BFF");
  const g = props.gaze ?? { x: 0, y: 0.2 };
  const shut = props.state === "sleeping" || props.state === "offline";
  const wide = props.state === "approval";
  const r = h * (wide ? 0.3 : 0.26);
  const dx = g.x * r * 0.45;
  const dy = g.y * r * 0.45;
  const eye = (cx: number) => (
    <g key={cx}>
      <ellipse cx={cx} cy={h / 2} rx={r} ry={shut ? r * 0.12 : r} fill="#FFFFFF" opacity={props.state === "offline" ? 0.4 : 1} />
      {shut ? null : <circle cx={cx + dx} cy={h / 2 + dy} r={r * 0.55} fill={iris} />}
    </g>
  );
  return (
    <svg width={w} height={h} viewBox={`0 0 ${w} ${h}`} role="img" aria-label={props.label} className={props.className} style={props.style}>
      {eye(w * 0.32)}
      {eye(w * 0.68)}
    </svg>
  );
}

const SWATCHES = ["#7A4BFF", "#2EC4B6", "#FF8FB1", "#F4B942", "#4D96FF", "#E5E5EA"];

export function FaceEditor(props: { value: FaceConfig | null; synthId: string; onChange: (face: FaceConfig) => void; lang?: string }) {
  const iris = pick(props.value, ["eyes", "iris", "color"], "#7A4BFF");
  return (
    <div className="face-editor-placeholder">
      <Face config={props.value} size={56} />
      <div className="swatches">
        {SWATCHES.map((c) => (
          <button
            key={c}
            type="button"
            className="swatch"
            aria-pressed={c === iris}
            style={{ background: c }}
            onClick={() => props.onChange({ ...(props.value ?? { v: 1 }), eyes: { ...((props.value?.eyes as object) ?? {}), iris: { color: c, size: 0.55 } } })}
          />
        ))}
      </div>
    </div>
  );
}
