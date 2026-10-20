// A measured stand-in for the real island, for review only (demo page and
// the editor's preview). It follows agent I's geometry: on a Mac with a
// notch the black shape is the hardware cutout plus two 76 pt ears, the face
// sits in the left ear, a status glyph in the right one, and nothing may be
// drawn over the cutout (the camera lives there). Without a notch it is a
// 184 pt pill as tall as the menu bar, face on the left.

import type { CSSProperties, ReactNode } from "react";
import { Face, faceWidth } from "./Face";
import type { Vec } from "./motion";
import type { FaceConfig, FaceState, IdleEmote } from "./schema";

export const EAR = 76;
export const NOTCH = 186;
export const PILL = 184;

export interface FixtureProps {
  face: FaceConfig;
  state: FaceState;
  /** Menu bar height in px: 32, 34 or 37 on current MacBooks. */
  height: number;
  notch?: boolean;
  /** Light menu bar (the black shape then stands out the most). */
  light?: boolean;
  gaze?: Vec | null;
  level?: number;
  still?: boolean;
  reduced?: boolean;
  emote?: { kind: IdleEmote; nonce: number } | null;
  /** With `still`: a blink closure 0..1 (review screenshots). */
  blink?: number;
  /** The whole menu bar width; default: just around the island. */
  barWidth?: number;
  /** Shows the cutout outline and the face slot (review aid). */
  guides?: boolean;
  /** Only the left ear and the start of the cutout (narrow previews). */
  crop?: boolean;
  status?: ReactNode;
}

export function NotchFixture(props: FixtureProps) {
  const { height: h, notch = true, light = false, guides = false } = props;
  const crop = props.crop ?? false;
  const shapeW = crop ? EAR + 28 : notch ? EAR * 2 + NOTCH : PILL;
  const slotW = notch ? EAR : Math.round(PILL * 0.42);
  const barW = Math.max(shapeW, props.barWidth ?? shapeW + 80);
  const radius = notch ? Math.round(h * 0.3) : Math.round(h / 2);
  const bar: CSSProperties = {
    position: "relative",
    width: barW,
    height: h,
    background: light ? "rgba(246, 243, 247, 0.92)" : "rgba(28, 26, 33, 0.92)",
    display: "flex",
    justifyContent: "center",
  };
  return (
    <div style={bar} className="fx-bar">
      <div className="fx-shape" style={{ width: shapeW, height: h, borderRadius: crop ? `0 0 0 ${radius}px` : `0 0 ${radius}px ${radius}px` }}>
        <div className="fx-slot" style={{ width: slotW, height: h }} data-guides={guides || undefined}>
          <Face
            config={props.face}
            state={props.state}
            gaze={props.gaze}
            level={props.level}
            size={h}
            still={props.still}
            reducedMotion={props.reduced}
            emote={props.emote}
            blink={props.blink}
            autoSleep={false}
            style={{ width: Math.min(slotW, faceWidth(h)), height: h }}
          />
        </div>
        {crop ? (
          <div className="fx-cutout" style={{ width: 28, height: h }} data-guides />
        ) : (
          <>
            {notch ? <div className="fx-cutout" style={{ width: NOTCH, height: h }} data-guides={guides || undefined} /> : <div style={{ flex: 1 }} />}
            <div className="fx-status" style={{ width: notch ? EAR : 30, height: h }}>
              {props.status ?? <span className="fx-dot" />}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
