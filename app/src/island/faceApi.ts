// The one door to the face module (island/face/**: renderer, schema,
// presets, editor, its own strings). Everything else in the app imports
// the face from here.

export { Face, FaceEditor, faceFor, faceWidth, presetFace } from "./face";
export type { FaceConfig, FaceState } from "./face";
