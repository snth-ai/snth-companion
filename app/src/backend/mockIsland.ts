// The island in the browser mock: the Island screen's state and the
// island window's view, staged from the URL and played by the review
// controls.
//
//   index.html?screen=island&island=off|on|outdated|taken|micdenied|two|nolink|
//        stt|sttdl|sttspace|sttold|sttintel
//        (the Island screen. off = the default, a fresh install; on = on,
//         Mia's face set; outdated = Mia's synth is older than the island;
//         taken = another app holds the voice key; micdenied = the
//         microphone was refused; two = Mia and Maya, Maya listens to the
//         keys; nolink = on, but Mia is offline, so the keys are released.
//         Speech recognition: stt = on this computer, the model here;
//         sttdl = its download at 34%; sttspace = the disk was too full;
//         sttold = on, but Mia does not take text yet; sttintel = a Mac
//         without Apple silicon, where it is not available. Wake by name:
//         wake = on and listening; wakedl = its model downloading;
//         wakecall = paused, another app uses the microphone; wakebt =
//         paused, the only microphone is in AirPods)
//   island.html?island=idle|hover|listening|screen|sending|accepted|thinking|
//        working|driving|done|error|approval|text|offline|outdated|
//        micdenied|asking|paused|two|shot|live|liveloading|livesent|
//        heardnothing|offer|offerdl|offerready|offerfail|wakeidle|
//        wakepaused|woke|wokename
//        (live = a hold with its words recognized on this computer;
//         liveloading = the same while the model is still loading;
//         heardnothing = a hold dropped as silence by the voice detector;
//         livesent = the text going after the release; offer = Mia's offer
//         of on-device speech-to-text; offerdl, offerready, offerfail = the
//         model's download after "Turn on", done, failed; wakeidle = wake
//         by name listening (the dot in the right ear); wakepaused = it
//         paused for a call; woke = "Мия, ..." heard, the request being
//         recorded with its words; wokename = "Мия" alone, waiting)
//        (the island window on a desktop; &notch=0 draws a Mac without a
//         notch, &os=windows a Windows screen: the island floats at the
//         bottom above a taskbar and grows upwards, &taskbar=top puts the
//         taskbar on the top edge and the island under it. The pointer over the desktop
//         is the gaze; the strip under it plays a hold, a line of text, a
//         request, an answer. &full=1 starts in full screen: the island
//         hides, and the pointer at the desktop's top edge brings the menu
//         bar and the island back, as the shell does. &face=<preset>
//         puts that preset on Mia instead of her own look, e.g.
//         &face=buddy.)
//
// Nothing here touches the network or the computer.

import { presetFace } from "../island/faceApi";
import type {
  ApprovalRequest,
  FaceConfig,
  FaceSetResult,
  HotkeyKind,
  Hotkeys,
  IslandNotice,
  IslandPlacement,
  IslandRect,
  IslandSignal,
  IslandState,
  IslandView,
  LinkBrief,
  SttCard,
  SttEngine,
  SttMode,
  SttModelStatus,
  UiState,
  WakeStatus,
} from "./types";

/** The two model files, as the app pins them (stt/model.rs). */
export const STT_SIZE = 574_041_195 + 885_098;
/** Parakeet's files: the encoder, the decoder, the tokens, the voice
 * detector. */
export const PARAKEET_SIZE = 652_183_999 + 18_202_004 + 93_939 + 885_098;
/** On Windows the voice detector is the Silero ONNX file (model.rs
 * SILERO_ONNX), Parakeet the only engine. */
export const PARAKEET_WIN_SIZE = 652_183_999 + 18_202_004 + 93_939 + 2_327_524;
/** Wake by name's files (wake/mod.rs): the voice detector, whisper tiny. */
export const WAKE_SIZE = 885_098 + 32_152_673;

const params = new URLSearchParams(window.location.search);

export function defaultHotkeys(os: string): Hotkeys {
  return os === "macos"
    ? { voice: "Alt+Space", screen_voice: "Alt+Shift+Space", text: "Control+Alt+Space" }
    : { voice: "Control+Alt+Space", screen_voice: "Control+Alt+Shift+Space", text: "Control+Alt+Enter" };
}

/** An accelerator in one comparable form (as the core compares them). */
export function canonical(accelerator: string): string {
  const mods = new Set<string>();
  let key = "";
  for (const raw of accelerator.split("+").map((t) => t.trim()).filter(Boolean)) {
    const t = raw.toLowerCase();
    if (t === "alt" || t === "option") mods.add("alt");
    else if (t === "shift") mods.add("shift");
    else if (t === "control" || t === "ctrl") mods.add("control");
    else if (["command", "cmd", "super", "meta", "commandorcontrol", "cmdorctrl"].includes(t)) mods.add("super");
    else key = t.replace(/^key/, "");
  }
  if (!key && mods.size === 0) return "";
  return [...[...mods].sort(), key].join("+");
}

/** The names the app answers to (wake/names.rs, `Names::shown`), for a
 * Latin name. */
export function wakeNames(name: string, phrase: string): string[] {
  const out = [name];
  const low = name.toLowerCase();
  if (low === "mia") out.push("Мия", "Миа");
  if (low === "maya") out.push("Майя");
  if (phrase.trim()) out.push(phrase.trim());
  return out;
}

/** The face Mia set for herself in the mock: her preset, a little
 * changed, or the preset named by &face= (review). */
const MIA_FACE: FaceConfig = (() => {
  const named = params.get("face");
  if (named) return presetFace(named);
  const face = presetFace("mia");
  face.accessory = "none";
  return face;
})();

/** A model's status in a staged scenario (stt, sttdl, sttspace...). */
function sttModelOf(scenario: string, size: number): SttModelStatus {
  return scenario === "stt" || scenario === "sttold" || scenario === "sttparakeet"
    ? { state: "ready" }
    : scenario === "sttdl"
      ? { state: "downloading", done: Math.round(size * 0.34), total: size }
      : scenario === "sttspace"
        ? { state: "failed", code: "no_space", detail: `${size + 300 * 1024 * 1024} ${412 * 1024 * 1024}` }
        : { state: "missing" };
}

export function stagedIslandState(os: string, links: LinkBrief[]): IslandState {
  const scenario = params.get("island") ?? "off";
  const enabled = scenario !== "off";
  const main = scenario === "two" && links.some((l) => l.synth_id === "maya") ? "maya" : "";
  const supported: Record<string, boolean> = {};
  for (const l of links) supported[l.synth_id] = !(scenario === "outdated" && l.synth_id === "mia");
  const mainId = main || links[0]?.synth_id || null;
  const stt = scenario.startsWith("stt") && scenario !== "sttintel";
  // Windows: Parakeet alone (its files, whatever the settings say).
  const win = os === "windows";
  const pkSize = win ? PARAKEET_WIN_SIZE : PARAKEET_SIZE;
  const pkModel: SttModelStatus = win && stt ? sttModelOf(scenario, pkSize) : scenario === "sttparakeet" ? { state: "ready" } : { state: "missing" };
  const deviceStt: Record<string, boolean> = {};
  for (const l of links) deviceStt[l.synth_id] = scenario !== "sttold";
  const model: SttModelStatus = scenario === "sttparakeet" ? { state: "missing" } : sttModelOf(scenario, STT_SIZE);
  const wakeOn = scenario.startsWith("wake") && scenario !== "wakeintel";
  const wakeModel: SttModelStatus =
    scenario === "wakedl" ? { state: "downloading", done: Math.round(WAKE_SIZE * 0.4), total: WAKE_SIZE } : wakeOn ? { state: "ready" } : { state: "missing" };
  const wakeStatus: WakeStatus = !wakeOn
    ? { state: "off" }
    : scenario === "wakedl"
      ? { state: "waiting", need: "model" }
      : scenario === "wakecall"
        ? { state: "paused", reason: "mic_busy" }
        : scenario === "wakebt"
          ? { state: "paused", reason: "bt_mic" }
          : { state: "listening" };
  return {
    available: os !== "linux",
    settings: {
      enabled,
      main_synth: main,
      hotkeys: defaultHotkeys(os),
      stt: stt ? "device" : "synth",
      stt_offer: stt ? "accepted" : "pending",
      stt_engine: scenario === "sttparakeet" ? "parakeet" : "whisper",
      wake: wakeOn,
      wake_phrase: "",
      stt_tone: true,
    },
    main_synth: mainId,
    faces: scenario === "off" ? {} : { mia: MIA_FACE },
    supported,
    device_stt: deviceStt,
    stt: {
      available: (os === "macos" && scenario !== "sttintel") || win,
      choices: win ? ["parakeet"] : ["whisper", "parakeet"],
      engine: win || scenario === "sttparakeet" ? "parakeet" : "whisper",
      model: win || scenario === "sttparakeet" ? pkModel : model,
      size: win || scenario === "sttparakeet" ? pkSize : STT_SIZE,
      engines: {
        whisper: { model: win ? { state: "missing" } : model, size: STT_SIZE },
        parakeet: { model: pkModel, size: pkSize },
      },
    },
    wake: {
      ...wakeStatus,
      available: os === "macos" && scenario !== "sttintel" && scenario !== "wakeintel",
      model: wakeModel,
      size: WAKE_SIZE,
      names: wakeNames("Mia", ""),
      wakes: wakeOn ? 3 : 0,
      dropped: wakeOn ? 1 : 0,
    },
    defaults: defaultHotkeys(os),
    keys_active: enabled && scenario !== "nolink" && links.length > 0,
    key_problems: scenario === "taken" ? { voice: "taken" } : {},
    mic: scenario === "micdenied" ? "denied" : scenario === "asking" ? "not_asked" : "granted",
  };
}

/** The Island screen's side of the mock. The state lives in the main
 * mock's UiState. */
export class MockIslandScreen {
  constructor(
    private state: () => UiState,
    private changed: () => Promise<UiState>,
  ) {}

  set(enabled: boolean | null, mainSynth: string | null): Promise<UiState> {
    const isl = this.state().island;
    if (enabled !== null) {
      if (enabled && !isl.available) return Promise.reject("unsupported");
      isl.settings.enabled = enabled;
      isl.keys_active = enabled && this.state().core.links.some((l) => l.state === "linked");
    }
    if (mainSynth !== null) {
      isl.settings.main_synth = mainSynth;
      isl.main_synth = mainSynth || this.state().core.links[0]?.synth_id || null;
    }
    return this.changed();
  }

  setHotkey(which: HotkeyKind, accelerator: string): Promise<UiState> {
    const isl = this.state().island;
    const want = canonical(accelerator);
    if (want) {
      for (const other of ["voice", "screen_voice", "text"] as HotkeyKind[]) {
        if (other !== which && canonical(isl.settings.hotkeys[other]) === want) return Promise.reject(`conflict:${other}`);
      }
      if (canonical(this.state().prefs.stop_shortcut) === want) return Promise.reject("conflict:stop");
      // One combination is "taken by another app", for review.
      if (want === "alt+shift+q" || want === "super+shift+q") return Promise.reject("shortcut_unavailable");
    }
    isl.settings.hotkeys[which] = accelerator;
    delete isl.key_problems[which];
    return this.changed();
  }

  resetHotkey(which: HotkeyKind): Promise<UiState> {
    return this.setHotkey(which, this.state().island.defaults[which]);
  }

  async faceSet(synthId: string, face: FaceConfig): Promise<FaceSetResult> {
    await new Promise((r) => window.setTimeout(r, 250));
    if (JSON.stringify(face).length > 4096) return { result: "invalid", reason: "a face is at most 4 KB" };
    if (params.get("faceset") === "invalid") return { result: "invalid", reason: "eyes.shape: one of round, oval, almond, droopy, cat, dot, line, star, heart, pill" };
    this.state().island.faces[synthId] = face;
    void this.changed();
    return { result: "saved", face };
  }

  faceRefresh(): Promise<UiState> {
    return this.changed();
  }

  // --- speech recognition ---

  private sttTimers: Record<SttEngine, number> = { whisper: 0, parakeet: 0 };

  /** The engine that runs for a choice (the settings' when none):
   * Parakeet on Windows, whatever was chosen (stt/mod.rs `effective`). */
  private sttEngine(engine?: SttEngine): SttEngine {
    const isl = this.state().island;
    const e = engine ?? isl.settings.stt_engine;
    return isl.stt.choices.includes(e) ? e : isl.stt.choices[0]!;
  }

  /** The chosen engine's model is the one `model` shows. */
  private sttMirror() {
    const stt = this.state().island.stt;
    stt.engine = this.sttEngine();
    const chosen = stt.engines[stt.engine];
    stt.model = chosen.model;
    stt.size = chosen.size;
  }

  private sttChanged(): Promise<UiState> {
    this.sttMirror();
    return this.changed();
  }

  sttSet(mode: SttMode): Promise<UiState> {
    const isl = this.state().island;
    if (mode === "device" && !isl.stt.available) return Promise.reject("unsupported");
    isl.settings.stt = mode;
    if (isl.settings.stt_offer === "pending" || isl.settings.stt_offer === "shown") isl.settings.stt_offer = mode === "device" ? "accepted" : "declined";
    if (mode === "device") return this.sttDownload().catch(() => this.sttChanged());
    return this.sttChanged();
  }

  sttEngineSet(engine: SttEngine): Promise<UiState> {
    const isl = this.state().island;
    if (!isl.stt.available || !isl.stt.choices.includes(engine)) return Promise.reject("unsupported");
    isl.settings.stt_engine = engine;
    if (isl.settings.stt === "device") return this.sttDownload(engine).catch(() => this.sttChanged());
    return this.sttChanged();
  }

  sttToneSet(on: boolean): Promise<UiState> {
    this.state().island.settings.stt_tone = on;
    return this.changed();
  }

  /** A pretend download: about 6 s to the end, then the check. */
  sttDownload(engine?: SttEngine): Promise<UiState> {
    const isl = this.state().island;
    const e = this.sttEngine(engine);
    const slot = isl.stt.engines[e];
    if (slot.model.state === "ready" || slot.model.state === "downloading") return this.sttChanged();
    if (params.get("sttnospace") === "1") {
      slot.model = { state: "failed", code: "no_space", detail: `${slot.size + 300 * 1024 * 1024} ${412 * 1024 * 1024}` };
      void this.sttChanged();
      return Promise.reject("no_space");
    }
    slot.model = { state: "downloading", done: 0, total: slot.size };
    window.clearInterval(this.sttTimers[e]);
    this.sttTimers[e] = window.setInterval(() => {
      const m = slot.model;
      if (m.state !== "downloading") return window.clearInterval(this.sttTimers[e]);
      const done = Math.min(m.total, m.done + slot.size / 25);
      if (done >= m.total) {
        window.clearInterval(this.sttTimers[e]);
        slot.model = { state: "verifying" };
        window.setTimeout(() => {
          slot.model = { state: "ready" };
          void this.sttChanged();
        }, 800);
      } else slot.model = { state: "downloading", done, total: m.total };
      void this.sttChanged();
    }, 250);
    return this.sttChanged();
  }

  sttCancel(engine?: SttEngine): Promise<UiState> {
    const isl = this.state().island;
    const e = this.sttEngine(engine);
    window.clearInterval(this.sttTimers[e]);
    isl.stt.engines[e].model = { state: "missing" };
    return this.sttChanged();
  }

  sttDelete(engine?: SttEngine): Promise<UiState> {
    const isl = this.state().island;
    const e = this.sttEngine(engine);
    isl.stt.engines[e].model = { state: "missing" };
    if (e === this.sttEngine()) isl.settings.stt = "synth";
    return this.sttChanged();
  }

  // --- wake by name ---

  private wakeTimer = 0;

  /** The status the listener would have now. */
  private wakeStatus(): WakeStatus {
    const isl = this.state().island;
    if (!isl.settings.wake) return { state: "off" };
    if (isl.wake.model.state !== "ready") return { state: "waiting", need: "model" };
    return { state: "listening" };
  }

  private setWake(status: WakeStatus) {
    const w = this.state().island.wake;
    delete (w as { need?: string }).need;
    delete (w as { reason?: string }).reason;
    Object.assign(w, status);
  }

  wakeSet(on: boolean): Promise<UiState> {
    const isl = this.state().island;
    if (on && !isl.wake.available) return Promise.reject("unsupported");
    isl.settings.wake = on;
    this.setWake(this.wakeStatus());
    if (on) return this.wakeDownload();
    return this.changed();
  }

  wakePhrase(phrase: string): Promise<UiState> {
    const isl = this.state().island;
    isl.settings.wake_phrase = phrase.trim().split(/\s+/).join(" ").slice(0, 40);
    isl.wake.names = wakeNames("Mia", isl.settings.wake_phrase);
    return this.changed();
  }

  /** A pretend download: about 2 s. */
  wakeDownload(): Promise<UiState> {
    const isl = this.state().island;
    if (isl.wake.model.state === "ready" || isl.wake.model.state === "downloading") return this.changed();
    isl.wake.model = { state: "downloading", done: 0, total: WAKE_SIZE };
    this.setWake(this.wakeStatus());
    window.clearInterval(this.wakeTimer);
    this.wakeTimer = window.setInterval(() => {
      const m = isl.wake.model;
      if (m.state !== "downloading") return window.clearInterval(this.wakeTimer);
      const done = Math.min(m.total, m.done + WAKE_SIZE / 8);
      isl.wake.model = done >= m.total ? { state: "ready" } : { state: "downloading", done, total: m.total };
      if (done >= m.total) window.clearInterval(this.wakeTimer);
      this.setWake(this.wakeStatus());
      void this.changed();
    }, 250);
    return this.changed();
  }

  wakeDownloadCancel(): Promise<UiState> {
    window.clearInterval(this.wakeTimer);
    this.state().island.wake.model = { state: "missing" };
    this.setWake(this.wakeStatus());
    return this.changed();
  }

  wakeDelete(): Promise<UiState> {
    const isl = this.state().island;
    isl.wake.model = { state: "missing" };
    isl.settings.wake = false;
    this.setWake({ state: "off" });
    return this.changed();
  }

  /** The synth designs its own face a moment later (a new look arrives). */
  async askDesign(synthId: string): Promise<void> {
    await new Promise((r) => window.setTimeout(r, 400));
    if (!this.state().island.supported[synthId]) throw "outdated";
    window.setTimeout(() => {
      const face = presetFace("star");
      face.accessory = "halo";
      this.state().island.faces[synthId] = face;
      void this.changed();
    }, 2500);
  }
}

// --- the island window ---

/** The stage's desktop (IslandStage), in points. */
export const STAGE_W = 1512;
export const STAGE_H = 520;
/** The Windows taskbar on the stage. */
export const STAGE_TASKBAR = 48;

function placement(os: string): IslandPlacement {
  if (os === "windows") {
    // As geometry.rs places it: a floating 84 x 28 pill 8 points above the
    // taskbar (or under one on the top edge), in a 480 x 300 window kept
    // inside the work area.
    const top = params.get("taskbar") === "top";
    const w = 480;
    const h = 300;
    const pill = { x: w / 2 - 42, y: top ? 8 : h - 8 - 28, w: 84, h: 28 };
    return {
      window: { x: STAGE_W / 2 - w / 2, y: top ? STAGE_TASKBAR : STAGE_H - STAGE_TASKBAR - h, w, h },
      notch: pill,
      pill,
      real_notch: false,
      edge: top ? "top" : "bottom",
      floating: true,
    };
  }
  const real = os === "macos" && params.get("notch") !== "0";
  const notchW = real ? 185 : 184;
  const notchH = real ? 32 : os === "macos" ? 25 : 24;
  const w = 480;
  const ear = 84;
  return {
    window: { x: 1512 / 2 - w / 2, y: 0, w, h: 300 },
    notch: { x: w / 2 - notchW / 2, y: 0, w: notchW, h: notchH },
    pill: { x: w / 2 - notchW / 2 - ear, y: 0, w: notchW + 2 * ear, h: notchH },
    real_notch: real,
    edge: "top",
    floating: false,
  };
}

const APPROVAL: ApprovalRequest = {
  id: 1,
  capability: "cookies",
  scope: "booking.com",
  summary: "hand the synth your browser cookies for booking.com",
  reason: "To check your reservation for Friday.",
  details: [],
  remember: "any",
  synth: "Mia",
  asked_at_ms: Date.now(),
  expires_at_ms: Date.now() + 55_000,
};

export class MockIslandWindow {
  private listeners = new Set<(s: IslandSignal) => void>();
  view: IslandView;
  private face: { x: number; y: number } | null = null;
  private timers: number[] = [];

  constructor() {
    const os = params.get("os") ?? "macos";
    const lang = params.get("lang") === "ru" ? "ru" : "en";
    const scenario = params.get("island") ?? "idle";
    const synths = [
      { id: "mia", name: "Mia", online: !["offline"].includes(scenario) },
      ...(scenario === "two" ? [{ id: "maya", name: "Maya", online: true }] : []),
    ];
    const now = Date.now();
    const notice = (code: string, detail = ""): IslandNotice => ({ code, detail, at_ms: now });
    this.view = {
      os,
      lang,
      placement: placement(os),
      synth: synths[0]!,
      face: MIA_FACE,
      supported: scenario !== "outdated",
      synths,
      paused: scenario === "paused",
      driving: scenario === "driving",
      task: scenario === "driving" ? "Mia: flights" : null,
      hold: scenario === "listening" ? "voice" : scenario === "screen" ? "screen_voice" : null,
      hold_since_ms: now - 2400,
      text_open: scenario === "text",
      sending: scenario === "sending",
      message: null,
      notice: null,
      approval: scenario === "approval" ? { ...APPROVAL, asked_at_ms: now, expires_at_ms: now + 55_000 } : null,
      hover: scenario === "hover" || scenario === "two",
      hidden: false,
      hotkeys: defaultHotkeys(os),
      mic: scenario === "micdenied" ? "denied" : scenario === "asking" ? "not_asked" : "granted",
      transcript: null,
      stt_loading: scenario === "liveloading",
      stt_card: null,
      wake: scenario === "wakepaused" ? { state: "paused", reason: "mic_busy" } : scenario.startsWith("wake") || scenario.startsWith("woke") ? { state: "listening" } : { state: "off" },
    };
    const ru = lang === "ru";
    const spoken = ru
      ? "Найди мне рейс в Лиссабон на пятницу, утром, и проверь, есть ли багаж в цене"
      : "Find me a flight to Lisbon on Friday morning and check whether a bag is included";
    const msg = (stage: "accepted" | "working" | "done" | "error", summary = "") =>
      ({ synth_id: "mia", message_id: "m-1", stage, at_ms: stage === "accepted" && scenario === "thinking" ? now - 5000 : now, summary, channel: "telegram" }) as const;
    switch (scenario) {
      case "accepted":
      case "thinking":
        this.view.message = { ...msg("accepted") };
        break;
      case "working":
      case "driving":
        this.view.message = { ...msg("working") };
        break;
      case "done":
        this.view.message = { ...msg("done", lang === "ru" ? "Нашла рейс в пятницу в 9:40, 214 €. Подробности в Telegram." : "Found a Friday flight at 9:40, 214 €. Details are in Telegram.") };
        break;
      case "error":
        this.view.message = { ...msg("error", lang === "ru" ? "Не получилось открыть сайт авиакомпании." : "Could not open the airline's site.") };
        break;
      case "offline":
        this.view.notice = notice("offline", "Mia");
        break;
      case "outdated":
        this.view.notice = notice("outdated", "Mia");
        break;
      case "micdenied":
        this.view.notice = notice("mic_denied");
        break;
      case "asking":
        this.view.notice = notice("mic_asking");
        break;
      case "shot":
        this.view.notice = notice("shot_permission");
        break;
      case "live":
        this.view.hold = "voice";
        this.view.transcript = spoken.split(" ").slice(0, 9).join(" ");
        break;
      case "liveloading":
        this.view.hold = "voice";
        this.view.transcript = "";
        break;
      case "heardnothing":
        this.view.notice = notice("heard_nothing");
        break;
      case "livesent":
        this.view.sending = true;
        this.view.transcript = spoken + ".";
        break;
      case "offer":
        this.view.stt_card = { kind: "offer", size: STT_SIZE };
        break;
      case "offerdl":
        this.view.stt_card = { kind: "download", percent: 34 };
        break;
      case "offerready":
        this.view.stt_card = { kind: "ready" };
        break;
      case "offerfail":
        this.view.stt_card = { kind: "failed", code: "no_space" };
        break;
      case "woke":
        this.view.hold = "voice";
        this.view.wake = { state: "woke" };
        this.view.transcript = ru ? "Мия, найди билеты в Бангкок на" : "Mia, find me tickets to Bangkok for";
        break;
      case "wokename":
        this.view.hold = "voice";
        this.view.wake = { state: "woke" };
        this.view.transcript = ru ? "Мия." : "Mia.";
        break;
    }
    // Stills stay still: no notice expires while a reviewer looks.
    if (this.view.notice) this.view.notice.at_ms = now + 3_600_000;
    if (this.view.message && scenario !== "thinking") this.view.message.at_ms = scenario === "done" || scenario === "error" ? now + 3_600_000 : now;
    if (params.get("full") === "1") this.setFullScreen(true);
  }

  // --- full screen, as the shell plays it (island/mod.rs, poll) ---

  /** A full-screen app covers the screen. */
  fullScreen = false;
  /** Its menu bar is in (the pointer went to the top edge). */
  barShown = false;
  private pointerY = 999;
  private barTimer = 0;
  private fullTimer = 0;

  setFullScreen(on: boolean) {
    this.fullScreen = on;
    this.barShown = false;
    window.clearInterval(this.fullTimer);
    // Busy ends by time (the summary, a notice): look again now and then.
    if (on) this.fullTimer = window.setInterval(() => this.recompute(), 250);
    this.recompute(true);
  }

  private busy(): boolean {
    const v = this.view;
    const now = Date.now();
    const m = v.message;
    const fresh = m ? (m.stage === "accepted" ? now - m.at_ms < 1500 : (m.stage === "done" || m.stage === "error") && now - m.at_ms < 6000) : false;
    return Boolean(v.hold || v.text_open || v.sending || v.approval || fresh || (v.notice && now - v.notice.at_ms < 6000));
  }

  private recompute(force = false) {
    const p = this.view.placement!;
    // The bar comes a moment after the pointer reaches the edge and goes
    // once it leaves the bar (measured on macOS: about 0.25 s).
    if (this.fullScreen && !this.barShown && this.pointerY <= 2 && !this.barTimer) {
      this.barTimer = window.setTimeout(() => {
        this.barTimer = 0;
        if (this.pointerY <= 2) {
          this.barShown = true;
          this.recompute(true);
        }
      }, 250);
    }
    if (this.barShown && this.pointerY > p.notch.h) {
      this.barShown = false;
      force = true;
    }
    const held = this.view.hover && !this.view.hidden;
    const hidden = this.fullScreen && !this.barShown && !held && !this.busy();
    if (hidden !== this.view.hidden || force) {
      this.view.hidden = hidden;
      if (hidden) this.view.hover = false;
      this.changed();
    }
  }

  subscribe(listener: (s: IslandSignal) => void) {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  private send(signal: IslandSignal) {
    window.setTimeout(() => this.listeners.forEach((l) => l(signal)), 0);
  }

  private changed() {
    this.send({ type: "view", view: structuredClone(this.view) });
  }

  private later(ms: number, f: () => void) {
    this.timers.push(window.setTimeout(f, ms));
  }

  layout(_hit: IslandRect, x: number, y: number) {
    this.face = { x, y };
  }

  /** The reviewer's pointer over the stage, in window coordinates. */
  /** The pointer, window-local; `edgeY` is its distance from the top of
   * the screen (the menu bar comes in at 0), the window-local y when the
   * window is at the top. */
  pointer(x: number, y: number, edgeY = y) {
    this.pointerY = edgeY;
    if (this.fullScreen) this.recompute();
    if (!this.face || this.view.hidden) return;
    let dx = (x - this.face.x) / 600;
    let dy = (y - this.face.y) / 600;
    const len = Math.hypot(dx, dy);
    if (len > 1) {
      dx /= len;
      dy /= len;
    }
    this.send({ type: "gaze", x: dx, y: dy, agent: false });
  }

  setHover(hover: boolean) {
    if (this.view.hidden) hover = false;
    if (this.view.hover === hover) return;
    this.view.hover = hover;
    this.changed();
    if (this.fullScreen) this.recompute();
  }

  // --- the review strip ---

  /** A hold of the voice key: listening with a level, then the answer. */
  playHold(kind: "voice" | "screen_voice") {
    this.view.hold = kind;
    this.view.hold_since_ms = Date.now();
    this.view.message = null;
    this.view.notice = null;
    this.changed();
    for (let i = 0; i < 50; i++) this.later(i * 50, () => this.send({ type: "level", level: Math.abs(Math.sin(i / 3)) * 0.8 + Math.random() * 0.2 }));
    this.later(2600, () => {
      this.view.hold = null;
      this.send({ type: "level", level: 0 });
      this.playAnswer();
    });
  }

  /** A hold with on-device speech-to-text: the words come as the owner
   * talks, the text goes on release. */
  playLiveHold() {
    const ru = this.view.lang === "ru";
    const words = (
      ru
        ? "Найди мне рейс в Лиссабон на пятницу, утром, и проверь, есть ли багаж в цене."
        : "Find me a flight to Lisbon on Friday morning and check whether a bag is included."
    ).split(" ");
    this.view.hold = "voice";
    this.view.hold_since_ms = Date.now();
    this.view.message = null;
    this.view.notice = null;
    this.view.stt_card = null;
    this.view.transcript = "";
    this.changed();
    for (let i = 0; i < 80; i++) this.later(i * 50, () => this.send({ type: "level", level: Math.abs(Math.sin(i / 3)) * 0.8 + Math.random() * 0.2 }));
    // A step about every 0.6 s, a few words each.
    const steps = Math.ceil(words.length / 2);
    for (let k = 1; k <= steps; k++) {
      this.later(k * 600, () => {
        this.view.transcript = words.slice(0, k * 2).join(" ");
        this.changed();
      });
    }
    this.later(steps * 600 + 400, () => {
      this.view.hold = null;
      this.view.transcript = words.join(" ");
      this.send({ type: "level", level: 0 });
      this.playAnswer();
    });
  }

  /** Wake by name: "Мия, найди билеты в Бангкок", said, not held. The
   * island opens listening as for a hold, the words come, a second of
   * silence sends it (the name stripped), the answer follows. */
  playWake(nameOnly = false) {
    const ru = this.view.lang === "ru";
    const name = ru ? "Мия" : "Mia";
    const request = (ru ? "найди билеты в Бангкок на пятницу" : "find me tickets to Bangkok for Friday").split(" ");
    this.view.wake = { state: "woke" };
    this.view.hold = "voice";
    this.view.hold_since_ms = Date.now();
    this.view.message = null;
    this.view.notice = null;
    this.view.stt_card = null;
    this.view.transcript = `${name},`;
    this.changed();
    for (let i = 0; i < 70; i++) this.later(i * 50, () => this.send({ type: "level", level: nameOnly && i > 10 ? 0.05 : Math.abs(Math.sin(i / 3)) * 0.8 + Math.random() * 0.2 }));
    if (nameOnly) {
      // Nothing follows the name: back to sleep, nothing sent.
      this.later(3500, () => {
        this.view.hold = null;
        this.view.transcript = null;
        this.view.wake = { state: "listening" };
        this.send({ type: "level", level: 0 });
        this.changed();
      });
      return;
    }
    for (let k = 1; k <= Math.ceil(request.length / 2); k++) {
      this.later(k * 550, () => {
        this.view.transcript = `${name}, ${request.slice(0, k * 2).join(" ")}`;
        this.changed();
      });
    }
    this.later(3600, () => {
      this.view.hold = null;
      this.view.wake = { state: "listening" };
      const text = request.join(" ");
      this.view.transcript = text.charAt(0).toUpperCase() + text.slice(1) + ".";
      this.send({ type: "level", level: 0 });
      this.playAnswer();
    });
  }

  /** The listener pauses for a call, and comes back after it. */
  toggleWakePause() {
    this.view.wake = this.view.wake.state === "paused" ? { state: "listening" } : { state: "paused", reason: "mic_busy" };
    this.changed();
  }

  /** Escape or a click on the island during a wake: nothing is sent. */
  wakeCancel() {
    if (this.view.wake.state !== "woke") return Promise.resolve();
    this.timers.forEach((t) => window.clearTimeout(t));
    this.timers = [];
    this.view.hold = null;
    this.view.transcript = null;
    this.view.wake = { state: "listening" };
    this.send({ type: "level", level: 0 });
    this.changed();
    return Promise.resolve();
  }

  /** The synth's offer of on-device speech-to-text, on an idle island. */
  playOffer() {
    this.view.message = null;
    this.view.notice = null;
    this.view.stt_card = { kind: "offer", size: STT_SIZE };
    this.changed();
  }

  /** "Turn on" / "Not now" on the offer: on, the download's card. */
  sttOffer(accept: boolean) {
    if (!accept) {
      this.view.stt_card = null;
      this.changed();
      return Promise.resolve();
    }
    let pct = 0;
    const tick = () => {
      pct = Math.min(100, pct + 7);
      this.view.stt_card = pct < 100 ? ({ kind: "download", percent: pct } as SttCard) : { kind: "verifying" };
      this.changed();
      if (pct < 100) this.later(250, tick);
      else
        this.later(700, () => {
          this.view.stt_card = { kind: "ready" };
          this.changed();
          this.later(6000, () => {
            this.view.stt_card = null;
            this.changed();
          });
        });
    };
    tick();
    return Promise.resolve();
  }

  sttCardHide() {
    this.timers.forEach((t) => window.clearTimeout(t));
    this.timers = [];
    this.view.stt_card = null;
    this.changed();
    return Promise.resolve();
  }

  playText() {
    this.view.text_open = true;
    this.changed();
  }

  /** Sent: accepted, thinking, working, done with a summary. */
  playAnswer(fail = false) {
    const ru = this.view.lang === "ru";
    this.view.sending = true;
    this.view.message = null;
    this.changed();
    const stage = (next: "accepted" | "working" | "done" | "error", summary = "") => {
      this.view.message = { synth_id: "mia", message_id: "m-2", stage: next, at_ms: Date.now(), summary, channel: "telegram" };
      this.changed();
    };
    this.later(500, () => {
      this.view.sending = false;
      stage("accepted");
    });
    this.later(2200, () => stage("working"));
    this.later(6000, () =>
      fail
        ? stage("error", ru ? "Не получилось: сайт не открылся." : "That did not work: the site did not open.")
        : stage("done", ru ? "Готово: напоминание на завтра в 9:00 стоит." : "Done: the reminder for tomorrow at 9:00 is set."),
    );
  }

  playApproval() {
    this.view.approval = { ...APPROVAL, asked_at_ms: Date.now(), expires_at_ms: Date.now() + 55_000 };
    this.changed();
  }

  toggleDriving() {
    this.view.driving = !this.view.driving;
    this.view.task = this.view.driving ? "Mia: flights" : null;
    this.changed();
  }

  toggleOffline() {
    const s = this.view.synth;
    if (!s) return;
    s.online = !s.online;
    this.view.synths = this.view.synths.map((x) => (x.id === s.id ? { ...x, online: s.online } : x));
    this.view.notice = s.online ? null : { code: "offline", detail: s.name, at_ms: Date.now() };
    this.changed();
  }

  // --- the window's commands ---

  get(): Promise<IslandView> {
    return Promise.resolve(structuredClone(this.view));
  }

  textSubmit(text: string) {
    this.view.text_open = false;
    this.changed();
    if (text.trim()) this.playAnswer();
    return Promise.resolve();
  }

  textCancel() {
    this.view.text_open = false;
    this.changed();
    return Promise.resolve();
  }

  selectSynth(id: string) {
    const next = this.view.synths.find((s) => s.id === id);
    if (next) {
      this.view.synth = next;
      this.changed();
    }
    return Promise.resolve();
  }

  stop() {
    this.view.paused = true;
    this.changed();
    return Promise.resolve();
  }

  answerApproval() {
    this.view.approval = null;
    this.changed();
  }
}
