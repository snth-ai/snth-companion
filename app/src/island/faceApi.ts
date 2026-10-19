// The one door to the face module (island/face/**: renderer, schema,
// presets, editor, its own strings). Everything else in the app imports
// the face from here.

export { Face } from "./face/Face";
export { FaceEditor } from "./face/FaceEditor";
export { faceFor, presetFace } from "./face/schema";
export type { FaceConfig, FaceState } from "./face/schema";
