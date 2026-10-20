// The island in the browser mock: the Island screen's state and the
// island window's view, staged from the URL and played by the review
// controls.
//
//   index.html?screen=island&island=off|on|outdated|taken|micdenied|two|nolink
//        (the Island screen. off = the default, a fresh install; on = on,
//         Mia's face set; outdated = Mia's synth is older than the island;
//         taken = another app holds the voice key; micdenied = the
//         microphone was refused; two = Mia and Maya, Maya listens to the
//         keys; nolink = on, but Mia is offline, so the keys are released)
//   island.html?island=idle|hover|listening|screen|sending|accepted|thinking|
//        working|driving|done|error|approval|text|offline|outdated|
//        micdenied|asking|paused|two|shot
//        (the island window on a desktop; &notch=0 draws a Mac without a
//         notch, &os=windows a Windows screen. The pointer over the desktop
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
  UiState,
} from "./types";

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

/** The face Mia set for herself in the mock: her preset, a little
 * changed, or the preset named by &face= (review). */
const MIA_FACE: FaceConfig = (() => {
  const named = params.get("face");
  if (named) return presetFace(named);
  const face = presetFace("mia");
  face.accessory = "none";
  return face;
})();

export function stagedIslandState(os: string, links: LinkBrief[]): IslandState {
  const scenario = params.get("island") ?? "off";
  const enabled = scenario !== "off";
  const main = scenario === "two" && links.some((l) => l.synth_id === "maya") ? "maya" : "";
  const supported: Record<string, boolean> = {};
  for (const l of links) supported[l.synth_id] = !(scenario === "outdated" && l.synth_id === "mia");
  const mainId = main || links[0]?.synth_id || null;
  return {
    available: os !== "linux",
    settings: { enabled, main_synth: main, hotkeys: defaultHotkeys(os) },
    main_synth: mainId,
    faces: scenario === "off" ? {} : { mia: MIA_FACE },
    supported,
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

function placement(os: string): IslandPlacement {
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
    };
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
  pointer(x: number, y: number) {
    this.pointerY = y;
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
