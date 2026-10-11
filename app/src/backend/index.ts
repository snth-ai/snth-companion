import type { Backend } from "./types";
import { getMock, type MockBackend } from "./mock";
import { tauriBackend } from "./tauri";

/** Inside the Tauri window the real shell answers. In a plain browser
 * (or with VITE_MOCK=1) the in-memory mock does, so every screen can be
 * reviewed without the app. */
export const isMock = !("__TAURI_INTERNALS__" in window) || import.meta.env.VITE_MOCK === "1";

export const backend: Backend = isMock ? getMock() : tauriBackend;

/** The mock's simulation controls. Only touch this when `isMock`. */
export const mockBackend: MockBackend = isMock ? getMock() : (undefined as unknown as MockBackend);

export * from "./types";
