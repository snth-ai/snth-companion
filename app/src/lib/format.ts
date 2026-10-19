import type { AppView, Entry, Grant, Settings } from "../backend/types";
import type { Lang, Strings } from "../i18n";

const locale = (lang: Lang) => (lang === "ru" ? "ru-RU" : "en-US");

export function formatBytes(bytes: number, s: Strings, lang: Lang): string {
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1024 && unit < s.units.bytes.length - 1) {
    value /= 1024;
    unit += 1;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  const text = new Intl.NumberFormat(locale(lang), { maximumFractionDigits: digits }).format(value);
  return `${text} ${s.units.bytes[unit]}`;
}

export function formatClock(unixSeconds: number, lang: Lang): string {
  return new Intl.DateTimeFormat(locale(lang), { hour: "numeric", minute: "2-digit" }).format(unixSeconds * 1000);
}

/** "14:30" today, "12 Oct, 14:30" on another day. */
export function formatUntil(unixSeconds: number, lang: Lang): string {
  const date = new Date(unixSeconds * 1000);
  if (date.toDateString() === new Date().toDateString()) return formatClock(unixSeconds, lang);
  return new Intl.DateTimeFormat(locale(lang), { day: "numeric", month: "short", hour: "numeric", minute: "2-digit" }).format(date);
}

/** A day, without the time: "3 Oct", and the year when it is not this one. */
export function formatDay(unixSeconds: number, lang: Lang): string {
  const date = new Date(unixSeconds * 1000);
  const sameYear = date.getFullYear() === new Date().getFullYear();
  return new Intl.DateTimeFormat(locale(lang), sameYear ? { day: "numeric", month: "short" } : { day: "numeric", month: "short", year: "numeric" }).format(date);
}

export function formatAgo(unixSeconds: number, lang: Lang): string {
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - unixSeconds);
  const rtf = new Intl.RelativeTimeFormat(locale(lang), { numeric: "auto" });
  if (diff < 60) return rtf.format(-diff, "second");
  if (diff < 3600) return rtf.format(-Math.floor(diff / 60), "minute");
  if (diff < 86400) return rtf.format(-Math.floor(diff / 3600), "hour");
  return rtf.format(-Math.floor(diff / 86400), "day");
}

export function dayLabel(unixSeconds: number, s: Strings, lang: Lang): string {
  const day = new Date(unixSeconds * 1000);
  const today = new Date();
  const yesterday = new Date(Date.now() - 86400_000);
  if (day.toDateString() === today.toDateString()) return s.activity.today;
  if (day.toDateString() === yesterday.toDateString()) return s.activity.yesterday;
  return new Intl.DateTimeFormat(locale(lang), { weekday: "long", day: "numeric", month: "long" }).format(day);
}

export function monthName(yyyyMm: string, lang: Lang): string {
  const [y, m] = yyyyMm.split("-").map(Number);
  if (!y || !m) return yyyyMm;
  const name = new Intl.DateTimeFormat(locale(lang), { month: "long" }).format(new Date(y, m - 1, 1));
  return name.charAt(0).toUpperCase() + name.slice(1);
}

/** The last one or two parts of a path: enough to recognize a folder
 * without the line being all path. The full path goes in a tooltip. */
export function shortPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 2) return path;
  return parts.slice(-2).join("/");
}

export function fileName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

export function parentFolder(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 1) return path;
  return parts.slice(Math.max(0, parts.length - 3), -1).join("/");
}

export function hostOf(url: string | null | undefined): string {
  if (!url) return "";
  return url.replace(/^[a-z]+:\/\//i, "").replace(/\/.*$/, "");
}

// --- the stop shortcut ---

const KEY_NAMES: Record<string, string> = {
  Period: ".",
  Comma: ",",
  Slash: "/",
  Backslash: "\\",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
  Escape: "Esc",
  ArrowUp: "Up",
  ArrowDown: "Down",
  ArrowLeft: "Left",
  ArrowRight: "Right",
};

/** "CommandOrControl+Alt+Period" -> ["Cmd", "Option", "."] on a Mac,
 * ["Ctrl", "Alt", "."] elsewhere. */
export function shortcutKeys(accelerator: string, os: string): string[] {
  const mac = os === "macos";
  return accelerator
    .split("+")
    .filter(Boolean)
    .map((token) => {
      switch (token.toLowerCase()) {
        case "commandorcontrol":
        case "cmdorctrl":
          return mac ? "⌘" : "Ctrl";
        case "command":
        case "cmd":
        case "super":
        case "meta":
          return mac ? "⌘" : "Win";
        case "control":
        case "ctrl":
          return mac ? "⌃" : "Ctrl";
        case "alt":
        case "option":
          return mac ? "⌥" : "Alt";
        case "shift":
          return mac ? "⇧" : "Shift";
        default:
          return KEY_NAMES[token] ?? token.replace(/^Key/, "").replace(/^Digit/, "");
      }
    });
}

export function shortcutText(accelerator: string, os: string): string {
  const keys = shortcutKeys(accelerator, os);
  return os === "macos" ? keys.join("") : keys.join("+");
}

/** A keydown -> an accelerator, or null while only modifiers are down.
 * "invalid" when a key was pressed without Ctrl, Alt or Cmd. */
export function acceleratorFromEvent(e: KeyboardEvent, os: string): string | null | "invalid" {
  if (["Control", "Alt", "Shift", "Meta", "OS"].includes(e.key)) return null;
  const mods: string[] = [];
  if (e.metaKey) mods.push(os === "macos" ? "Command" : "Super");
  if (e.ctrlKey) mods.push("Control");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (!e.metaKey && !e.ctrlKey && !e.altKey) return "invalid";
  if (!/^(Key[A-Z]|Digit\d|F\d{1,2}|Period|Comma|Slash|Backslash|Semicolon|Quote|BracketLeft|BracketRight|Minus|Equal|Backquote|Space|Escape|Arrow(Up|Down|Left|Right))$/.test(e.code)) {
    return "invalid";
  }
  return [...mods, e.code].join("+");
}

// --- one-line summaries of what is allowed ---

export function summarize(
  capability: "net" | "cookies" | "computer" | "files" | "apps" | "shell",
  settings: Settings,
  s: Strings,
  apps: AppView[] = [],
): string {
  const sum = s.status.sum;
  switch (capability) {
    case "shell":
      if (!settings.shell.enabled) return sum.off;
      return settings.shell.mode === "auto" ? sum.shellAuto : sum.shellAsk;
    case "apps": {
      if (!settings.apps.enabled) return sum.off;
      // Only what this computer has and the owner left switched on.
      const count = apps.filter((a) => a.on).length;
      if (count === 0) return sum.appsNone;
      const mode = settings.apps.mode === "ask" ? sum.appsAsk : settings.apps.mode === "ask_new" ? sum.appsAskNew : sum.appsAuto;
      return `${sum.apps(count)}, ${mode}`;
    }
    case "net":
      if (!settings.net.enabled) return sum.off;
      if (settings.net.mode === "all") return sum.netAll;
      if (settings.net.mode === "ask") return sum.netAsk;
      return sum.netList(settings.net.domains.length);
    case "cookies":
      if (!settings.cookies.enabled) return sum.off;
      if (settings.cookies.mode === "ask") return sum.cookiesAsk;
      if (settings.cookies.mode === "ask_new") return sum.cookiesAskNew;
      return sum.cookiesAuto;
    case "computer":
      if (!settings.computer.enabled) return sum.off;
      {
        const when =
          settings.computer.mode === "ask"
            ? sum.computerAsk
            : settings.computer.mode === "auto"
              ? sum.computerAuto
              : sum.computerAway(settings.computer.away_idle_minutes);
        const scope = settings.computer.apps;
        if (scope.policy === "only") return `${when}, ${sum.computerOnly(scope.only.length)}`;
        if (scope.policy === "except" && scope.except.length > 0) return `${when}, ${sum.computerExcept(scope.except.length)}`;
        return when;
      }
    case "files": {
      if (!settings.files.enabled) return sum.off;
      const count = settings.files.roots.length;
      if (count === 0) return sum.filesNone;
      const base = sum.files(count);
      return settings.files.ask_on_write ? `${base}, ${sum.filesAsk}` : base;
    }
  }
}

export function grantTime(grant: Grant, s: Strings, lang: Lang): string {
  if (grant.until === 0) return s.cookies.always;
  if (grant.until < 0) return s.cookies.session;
  return s.cookies.until(formatUntil(grant.until, lang));
}

// --- the activity journal in plain words ---

export type LineKind = "net" | "cookies" | "computer" | "files" | "apps" | "shell" | "link" | "app";

/** The browser a sign-in came from, as the core names it: a browser's
 * own name ("Chrome"), or "chrome-extension" when it came through the
 * SNTH Vault extension of a linked browser. */
function sourceLabel(source: string, s: Strings): string {
  const m = /^([a-z]+)-extension$/.exec(source);
  if (!m) return source;
  return s.logins.viaExtension(s.logins.browserName[m[1]!] ?? s.logins.browserName.browser!);
}

export interface Line {
  key: string;
  time: number;
  kind: LineKind;
  text: string;
  /** Extra facts in a quieter voice: size, count, the reason of a refusal. */
  note: string;
  refused: boolean;
  count: number;
  bytes: number;
  /** The synth the line is about; "" for the app's own lines. */
  synth: string;
}

function refusalReason(outcome: string, s: Strings): string {
  const why = s.journal.why;
  const o = outcome.toLowerCase();
  if (o.includes("no answer")) return why.noAnswer;
  if (o.includes("paused")) return why.paused;
  if (o.includes("turned off on this computer") || o.includes("is turned off")) return why.off;
  if (o.includes("too many notifications")) return why.tooMany;
  if (o.includes("not opened")) return why.notDocument;
  if (o.includes("no folder is shared")) return why.outsideFolders;
  if (o.includes("foreground delivery is turned off")) return why.screenOff;
  if (o.includes("foreground delivery is allowed only")) return why.screenAway;
  if (o.includes("did not allow") || o === "denied: owner") return why.youSaidNo;
  if (o.includes("not on the allowed list")) return why.notOnList;
  if (o.includes("local network")) return why.localNetwork;
  if (o.includes("traffic cap")) return why.cap;
  if (o.includes("metered")) return why.metered;
  if (o.includes("the look is the owner's")) return why.yourLook;
  if (o.includes("never allowed")) return why.never;
  if (o.includes("blocklist") || o.includes("denylist")) return why.blockedApp;
  if (o.includes("an app the owner excluded")) return why.appExcluded;
  if (o.includes("not one of the apps the owner chose")) return why.appNotChosen;
  if (o.includes("the whole screen, while apps are limited")) return why.wholeScreen;
  if (o.includes("no window named")) return why.noWindow;
  if (o.includes("a window of no known app")) return why.unknownApp;
  if (o.includes("while apps are limited") || o.includes("while only chosen apps are allowed") || o.includes("the app cannot place") || o.includes("while one is excluded"))
    return why.limitedApps;
  if (o.includes("only while you are away")) return why.away;
  if (o.includes("outside the shared folders")) return why.outsideFolders;
  if (o.includes("read-only")) return why.readOnly;
  return why.refused;
}

/** "www.youtube.com:443" -> "youtube.com"; an unusual port stays. */
function siteOf(target: string): string {
  const m = /^(.*):(\d+)$/.exec(target);
  const host = (m ? m[1]! : target).replace(/^www\./, "");
  const port = m ? m[2]! : "";
  return port && port !== "443" && port !== "80" ? `${host}:${port}` : host;
}

export function describeEntry(entry: Entry, s: Strings, lang: Lang): Omit<Line, "key" | "count" | "synth"> {
  const j = s.journal;
  const ok = entry.outcome === "ok";
  const refused = entry.outcome.startsWith("denied");
  const size = entry.bytes ? formatBytes(entry.bytes, s, lang) : "";
  const base = { time: entry.time, refused, bytes: entry.bytes ?? 0 };
  const why = refused ? refusalReason(entry.outcome, s) : "";

  switch (entry.capability) {
    case "net": {
      const site = siteOf(entry.target);
      if (ok) return { ...base, kind: "net", text: j.netOk(site), note: size };
      if (refused) return { ...base, kind: "net", text: j.netRefused(site), note: why };
      if (entry.outcome.startsWith("aborted")) return { ...base, kind: "net", text: j.netCut(site), note: size };
      return { ...base, kind: "net", text: j.netFailed(site), note: "" };
    }
    case "cookies": {
      // The core writes "youtube.com (14 from Chrome)" on success.
      const m = /^(\S+) \((\d+) from (.+)\)$/.exec(entry.target);
      const site = m ? m[1]! : entry.target;
      if (ok) return { ...base, kind: "cookies", text: j.cookiesOk(site), note: m ? j.cookiesCount(Number(m[2]), sourceLabel(m[3]!, s)) : "" };
      return { ...base, kind: "cookies", text: j.cookiesRefused(site), note: why, refused: true };
    }
    // The vault and the browsers: linking a browser, removing a password.
    // Shown with the sign-ins (the Logins filter).
    case "vault": {
      const failed = !ok && !refused;
      if (entry.action === "vault.forget") {
        return { ...base, kind: "cookies", text: ok ? j.vaultForgot(entry.target) : j.vaultForgetFailed(entry.target), note: "" };
      }
      if (entry.action === "vault.unpair") return { ...base, kind: "cookies", text: j.browserUnlinked(entry.target), note: "" };
      if (ok) return { ...base, kind: "cookies", text: j.browserLinked(entry.target), note: "" };
      return { ...base, kind: "cookies", text: failed ? j.browserLinkFailed(entry.target) : j.browserLinkRefused(entry.target), note: why };
    }
    case "files": {
      const name = shortPath(entry.target);
      if (refused) return { ...base, kind: "files", text: j.fileRefused(name), note: why };
      if (!ok) return { ...base, kind: "files", text: j.fileFailed(name), note: "" };
      if (entry.action === "files.write") return { ...base, kind: "files", text: j.fileWrite(name), note: size };
      if (entry.action === "files.list") return { ...base, kind: "files", text: j.fileList(name), note: "" };
      return { ...base, kind: "files", text: j.fileRead(name), note: "" };
    }
    case "computer": {
      // The app's own doing, not the synth's: it installed its helper.
      if (entry.action === "helper.install") return { ...base, kind: "computer", text: j.helperInstall(entry.target), note: "" };
      if (entry.action === "helper.update") return { ...base, kind: "computer", text: j.helperUpdate(entry.target), note: "" };
      if (entry.action === "mcp.open") {
        if (ok) return { ...base, kind: "computer", text: j.taskStart, note: "" };
        if (refused) return { ...base, kind: "computer", text: j.taskRefused, note: why };
        return { ...base, kind: "computer", text: j.taskFailed, note: "" };
      }
      const what = j.tool[entry.target] ?? j.toolOther(entry.target);
      if (refused) return { ...base, kind: "computer", text: j.toolRefused(what), note: why };
      return { ...base, kind: "computer", text: what, note: "" };
    }
    case "apps": {
      // "calendar.list", "open.url"... The target is a site, a file, an
      // app or a shortcut name where there is one, and empty otherwise:
      // what a message or a note says is never in the journal.
      const what =
        entry.action === "shortcuts.run"
          ? j.appShortcut(entry.target)
          : entry.action === "open.url"
            ? j.appUrl(entry.target)
            : entry.action === "open.file"
              ? j.appFile(shortPath(entry.target))
              : entry.action === "open.app"
                ? j.appOpenApp(entry.target)
                : (j.app[entry.action] ?? j.appOther(entry.action));
      const appId = entry.action.split(".")[0] ?? "";
      const appTitle = s.apps.title[appId] ?? appId;
      if (refused) return { ...base, kind: "apps", text: j.appRefused(appTitle), note: why };
      if (!ok) {
        const notHere = entry.outcome.includes("not available") ? j.why.notHere : "";
        return { ...base, kind: "apps", text: j.appFailed(appTitle), note: notHere };
      }
      return { ...base, kind: "apps", text: what, note: entry.count === undefined ? "" : j.appCount(entry.count) };
    }
    case "shell": {
      // The command and how it ended; never what it printed.
      const cmd = entry.target.length > 120 ? `${entry.target.slice(0, 120)}...` : entry.target;
      const o = entry.outcome;
      const facts = [
        entry.exit_code !== undefined && entry.exit_code !== 0 ? j.shellExit(entry.exit_code) : "",
        entry.cwd ? j.shellIn(shortPath(entry.cwd)) : "",
        entry.duration_ms !== undefined && !refused ? j.shellTook((entry.duration_ms / 1000).toFixed(1)) : "",
        size,
      ].filter(Boolean);
      if (refused) return { ...base, kind: "shell", text: j.shellRefused(cmd), note: why };
      const text = o.startsWith("timed out")
        ? j.shellTimedOut(cmd)
        : o.startsWith("stopped")
          ? j.shellStopped(cmd)
          : o.startsWith("cancelled")
            ? j.shellCancelled(cmd)
            : o.startsWith("failed")
              ? j.shellFailed(cmd)
              : j.shellRan(cmd);
      return { ...base, kind: "shell", text, note: facts.join(", ") };
    }
    case "link":
      return { ...base, kind: "link", text: j.linkUp, note: "" };
    case "app":
      if (entry.action === "update") return { ...base, kind: "app", text: j.appUpdate(entry.target), note: "" };
      return { ...base, kind: "app", text: j.other(entry.action, entry.target), note: "" };
    default:
      return { ...base, kind: "link", text: j.other(entry.action, entry.target), note: refused ? why : "" };
  }
}

/** Newest first, with a run of the same line inside two minutes folded
 * into one ("Opened youtube.com, 12 times, 8 MB"). */
export function buildLines(entries: Entry[], s: Strings, lang: Lang): Line[] {
  const lines: Line[] = [];
  for (let i = entries.length - 1; i >= 0; i--) {
    const entry = entries[i]!;
    const d = describeEntry(entry, s, lang);
    const synth = entry.synth ?? "";
    const prev = lines[lines.length - 1];
    if (prev && prev.kind === d.kind && prev.text === d.text && prev.refused === d.refused && prev.synth === synth && prev.time - d.time < 120) {
      prev.count += 1;
      prev.bytes += d.bytes;
      if (d.kind === "net" && !d.refused && prev.bytes > 0) prev.note = formatBytes(prev.bytes, s, lang);
      continue;
    }
    lines.push({ ...d, synth, key: `${entry.time}-${i}`, count: 1 });
  }
  return lines;
}

export interface TodayCounters {
  netCount: number;
  netBytes: number;
  cookies: number;
  computer: number;
  files: number;
  apps: number;
  refused: number;
}

export function todayCounters(entries: Entry[]): TodayCounters {
  const midnight = new Date();
  midnight.setHours(0, 0, 0, 0);
  const from = midnight.getTime() / 1000;
  const c: TodayCounters = { netCount: 0, netBytes: 0, cookies: 0, computer: 0, files: 0, apps: 0, refused: 0 };
  for (const e of entries) {
    if (e.time < from) continue;
    if (e.outcome.startsWith("denied")) {
      c.refused += 1;
      continue;
    }
    if (e.outcome !== "ok" && !e.outcome.startsWith("aborted")) continue;
    if (e.capability === "net") {
      c.netCount += 1;
      c.netBytes += e.bytes ?? 0;
    } else if (e.capability === "cookies") c.cookies += 1;
    else if (e.capability === "computer" && e.action === "tools/call") c.computer += 1;
    else if (e.capability === "files") c.files += 1;
    else if (e.capability === "apps") c.apps += 1;
  }
  return c;
}
