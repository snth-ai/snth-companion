// The one door to the face module (island/face/**, built on its own
// branch). Until it is merged this points at a small placeholder with the
// same props; after the merge it points at the real Face and FaceEditor.

export { Face, FaceEditor } from "./FacePlaceholder";
export type { FaceState } from "./FacePlaceholder";
