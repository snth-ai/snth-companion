// The island face, for the rest of the app (agent I wires it in).

export { Face, COMPACT_BELOW, faceAspect, faceWidth, type FaceProps } from "./Face";
export { FaceEditor, type FaceEditorProps } from "./FaceEditor";
export { faceStrings, presetBlurb, presetTitle, type FaceStrings } from "./i18n";
export type { Vec as FaceGaze } from "./motion";
export { randomFace } from "./random";
export {
  DEFAULT_POOL,
  FACE_STATES,
  IDLE_EMOTES,
  MAX_FACE_BYTES,
  PRESETS,
  clampFace,
  defaultFaceFor,
  faceFor,
  findPreset,
  mergeFace,
  presetFace,
  validateFace,
  validatePatch,
  type FaceConfig,
  type FacePreset,
  type FaceResult,
  type FaceState,
  type IdleEmote,
} from "./schema";
