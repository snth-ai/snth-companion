// The request window. The app's own wording comes first and says what
// would happen; the synth's reason is shown after it, marked as the
// synth's words. "Don't allow" has the focus, a closed window is a
// refusal, and so is no answer within the countdown.
//
// For a change in the owner's apps the window also shows the thing
// itself, in full and as it will go out: who a message is for and every
// word of it, the title and time of an event, the name (and input) of a
// shortcut. Those facts come from the core, not from the synth's reason.

import { useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import type { AnswerDuration, ApprovalDetail, ApprovalRequest } from "../backend/types";
import { fileName, parentFolder } from "../lib/format";
import { useNow, useStore } from "../lib/store";
import { Segmented } from "../ui/controls";
import type { Strings } from "../i18n";

type Span = "once" | "hour" | "hours8" | "session" | "always";

const ANSWER: Record<Span, { duration: AnswerDuration; hours?: number }> = {
  once: { duration: "once" },
  hour: { duration: "hours", hours: 1 },
  hours8: { duration: "hours", hours: 8 },
  session: { duration: "session" },
  always: { duration: "always" },
};

interface Wording {
  title: string;
  body: string;
  spans: Span[];
  /** Shown when only "this time" is possible, to say why. */
  hint?: string;
  /** Another label for a span ("Always for +7 916..."). */
  labels?: Partial<Record<Span, string>>;
  /** The facts of the action, shown in full under the body. */
  details?: ApprovalDetail[];
  /** A request this version does not understand: Allow is not offered
   * as the obvious button. */
  unknown?: boolean;
  /** Who asks, when it is not the synth (the owner's own browser). The
   * synth's "says" block is not shown then: the synth said nothing. */
  from?: string;
  /** One more plain line under the body (what happens next). */
  next?: string;
}

/** The browser a request names: from the app's own list, never free text. */
function browserLabel(id: string, s: Strings): string {
  const known = Object.values(s.logins.browserName);
  return known.includes(id) ? id : (s.logins.browserName.browser ?? id);
}

function wording(req: ApprovalRequest, s: Strings, name: string, cookiesAskEveryTime: boolean): Wording {
  const t = s.approval;
  switch (req.capability) {
    case "net":
      // One page is many connections, so "this time only" would ask again
      // at once; the shortest useful answer is an hour.
      return { title: t.netTitle(req.scope), body: t.netBody, spans: ["hour", "hours8", "session", "always"] };
    case "cookies": {
      // When a linked browser will ask too, this window says so first:
      // the two questions read as one.
      const browser = req.details.find((d) => d.key === "browser_step")?.value;
      const next = browser ? t.cookiesStep(browserLabel(browser, s)) : undefined;
      return cookiesAskEveryTime
        ? { title: t.cookiesTitle(req.scope), body: t.cookiesBody(name), spans: ["once"], hint: t.onceOnly, next }
        : { title: t.cookiesTitle(req.scope), body: t.cookiesBody(name), spans: ["once", "session", "always"], next };
    }
    // The owner's own browser (its SNTH Vault extension) asks to be
    // linked. The scope is the browser's name from the app's own list.
    case "vault": {
      const browser = browserLabel(req.scope, s);
      return { title: t.vaultTitle(browser, name), body: t.vaultBody(name), spans: ["once"], from: t.browserAsks(browser), next: t.vaultNotYou };
    }
    case "computer":
      if (req.scope.startsWith("foreground:")) {
        return { title: t.fgTitle, body: t.fgBody, spans: ["once", "hour", "session", "always"] };
      }
      if (req.scope === "task") {
        return { title: t.taskTitle, body: t.taskBody(name), spans: ["once", "hour", "hours8", "always"] };
      }
      break;
    case "files":
      return { title: t.fileTitle(fileName(req.scope)), body: t.fileBody(parentFolder(req.scope)), spans: ["once", "session", "always"] };
    case "apps":
      return appsWording(req, s);
    case "shell": {
      // A command: the whole of it, the folder and the shell, from the
      // core. Only "always for this exact command" can be remembered, and
      // only when the core offers it.
      const a = t.apps;
      const always = req.remember === "always";
      return {
        title: a.shellTitle,
        body: a.shellBody,
        details: req.details,
        spans: always ? ["once", "always"] : ["once"],
        labels: { always: a.alwaysCommand },
        hint: always ? undefined : a.shellOnceOnly,
      };
    }
  }
  // Something this version does not know: never remembered.
  return { title: t.otherTitle, body: t.otherBody(req.capability, req.scope), spans: ["once"], unknown: true };
}

/** A change in the owner's apps. The scope is the kind of change
 * ("calendar.create", "messages.send:<recipient>"); what may be
 * remembered is the core's call (`remember`), never this window's. */
function appsWording(req: ApprovalRequest, s: Strings): Wording {
  const t = s.approval;
  const a = t.apps;
  const [kind, ...rest] = req.scope.split(":");
  const subject = rest.join(":");
  const value = (key: string) => req.details.find((d) => d.key === key)?.value ?? "";
  // The facts worth a line of their own (the subject is in the title).
  const without = (...keys: string[]) => req.details.filter((d) => !keys.includes(d.key));

  let spans: Span[] = req.remember === "any" ? ["once", "session", "always"] : req.remember === "always" ? ["once", "always"] : ["once"];
  let hint: string | undefined = req.remember === "never" ? t.onceOnly : undefined;
  let labels: Wording["labels"];
  let words: { title: string; body: string; details: ApprovalDetail[] };

  switch (kind) {
    case "messages.send":
      words = { title: a.messageTitle, body: a.messageBody(subject), details: req.details };
      labels = { always: a.alwaysFor(subject) };
      break;
    case "calendar.create":
      words = { title: a.eventTitle, body: "", details: req.details };
      break;
    case "reminders.create":
      words = { title: a.reminderTitle, body: "", details: req.details };
      break;
    case "reminders.complete":
      words = { title: a.reminderDoneTitle, body: a.reminderDoneBody, details: [] };
      break;
    case "notes.create":
      words = { title: a.noteTitle, body: "", details: req.details };
      break;
    case "shortcuts.run": {
      const input = value("input");
      words = { title: a.shortcutTitle(subject), body: input ? a.shortcutInputBody : a.shortcutBody, details: without("name") };
      if (input && req.remember === "never") hint = a.inputOnceOnly;
      break;
    }
    case "clipboard.write":
      words = { title: a.clipboardTitle, body: a.clipboardBody, details: req.details };
      break;
    case "open.url":
      words = { title: a.urlTitle(subject), body: a.urlBody, details: req.details };
      break;
    case "open.file":
      words = { title: a.fileTitle(fileName(value("path"))), body: a.fileBody, details: req.details };
      break;
    case "open.app":
      words = { title: a.appTitle(value("name") || subject), body: a.appBody, details: [] };
      break;
    default:
      // A kind of change this version does not know: shown as it is,
      // never remembered.
      words = { title: a.otherTitle, body: req.scope, details: req.details };
      spans = ["once"];
      hint = undefined;
      return { ...words, spans, unknown: true };
  }
  return { ...words, spans, hint, labels };
}

/** For the island: the request's title and the shortest answer that
 * allows it (the window's first choice), so the island can offer Allow
 * and Don't allow without the window. A request this version does not
 * understand is not offered there (`unknown`). */
export function quickAnswer(
  req: ApprovalRequest,
  s: Strings,
  name: string,
): { title: string; duration: AnswerDuration; hours?: number; unknown: boolean } {
  const words = wording(req, s, name, false);
  const first = ANSWER[words.spans[0] ?? "once"];
  return { title: words.title, duration: first.duration, hours: first.hours, unknown: Boolean(words.unknown) };
}

function Details(props: { items: ApprovalDetail[]; s: Strings }) {
  const a = props.s.approval.apps;
  if (props.items.length === 0) return null;
  return (
    <div className="exact">
      <div className="exact-label">{a.what}</div>
      <dl>
        {props.items.map((d) => (
          <div className="exact-row" key={d.key}>
            <dt>{a.detail[d.key] ?? d.key}</dt>
            <dd className={`selectable${d.key === "text" || d.key === "input" || d.key === "notes" ? " long" : ""}${d.key === "command" || d.key === "cwd" ? " code" : ""}`}>
              {d.key === "service" ? (a.service[d.value] ?? d.value) : d.value}
            </dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

export function ApprovalView() {
  const { state, s, name: firstName, act } = useStore();
  const t = s.approval;
  const [requests, setRequests] = useState<ApprovalRequest[]>([]);
  const [span, setSpan] = useState<Span>("once");
  const [sending, setSending] = useState(false);
  const denyRef = useRef<HTMLButtonElement>(null);
  const now = useNow(500);

  useEffect(() => {
    let alive = true;
    void backend
      .approvalsPending()
      .then((list) => {
        if (alive) setRequests(list);
      })
      .catch(() => undefined);
    const stop = backend.subscribe((event) => {
      if (event.type === "approval") {
        setRequests((list) => (list.some((r) => r.id === event.request.id) ? list : [...list, event.request]));
      } else if (event.type === "approval_closed") {
        setRequests((list) => list.filter((r) => r.id !== event.id));
      }
    });
    return () => {
      alive = false;
      stop();
    };
  }, []);

  // Requests whose time ran out leave the queue here too; the shell has
  // already refused them.
  const live = requests.filter((r) => r.expires_at_ms > now);
  const current = live[0];
  const currentId = current?.id;
  const askEveryTime = state.core.settings.cookies.mode === "ask";
  // Which synth asks: with several linked, the request says so.
  const name = current?.synth || firstName;
  const words = current ? wording(current, s, name, askEveryTime) : null;
  const firstSpan = words?.spans[0];

  useEffect(() => {
    if (currentId === undefined) return;
    setSpan(firstSpan ?? "once");
    setSending(false);
    denyRef.current?.focus();
  }, [currentId, firstSpan]);

  if (!current || !words) {
    return (
      <div className="approval">
        <div className="approval-empty">{t.empty}</div>
      </div>
    );
  }

  const total = current.expires_at_ms - current.asked_at_ms;
  const leftMs = Math.max(0, current.expires_at_ms - now);
  const seconds = Math.ceil(leftMs / 1000);
  const chosen = words.spans.includes(span) ? span : words.spans[0]!;

  const answer = (allow: boolean) => {
    if (sending) return;
    setSending(true);
    const a = ANSWER[chosen];
    void backend
      .approvalAnswer(current.id, allow, allow ? a.duration : "once", a.hours)
      .catch(() => undefined)
      .finally(() => setRequests((list) => list.filter((r) => r.id !== current.id)));
  };

  return (
    <div
      className="approval"
      onKeyDown={(e) => {
        if (e.key === "Escape") answer(false);
      }}
    >
      <div className="approval-head">
        <span className="avatar" aria-hidden="true">
          {(words.from ?? name).charAt(0).toUpperCase()}
        </span>
        <span className="approval-who">{words.from ?? t.asks(name)}</span>
        {live.length > 1 ? <span className="approval-queue">{t.queue(1, live.length)}</span> : null}
      </div>

      <div className="approval-body">
        <h1>{words.title}</h1>
        {words.body ? <p className="approval-what">{words.body}</p> : null}
        {words.next ? <p className="approval-next">{words.next}</p> : null}
        {words.details ? <Details items={words.details} s={s} /> : null}

        {words.from ? null : (
          <div className="says">
            <div className="says-label">{t.says(name)}</div>
            <div className="says-text selectable">{current.reason ? `"${current.reason}"` : t.noReason}</div>
            {current.reason ? <div className="says-note">{t.unverified}</div> : null}
          </div>
        )}

        {words.spans.length > 1 ? (
          <div className="howlong">
            <div className="howlong-label">{t.howLong}</div>
            <Segmented<Span>
              label={t.howLong}
              value={chosen}
              onChange={setSpan}
              options={words.spans.map((v) => ({ value: v, title: words.labels?.[v] ?? t[v] }))}
            />
          </div>
        ) : words.hint ? (
          <p className="section-note" style={{ marginTop: 12 }}>
            {words.hint}
          </p>
        ) : null}
      </div>

      <div className="approval-foot">
        <div className={`countdown${seconds <= 10 ? " late" : ""}`} aria-hidden="true">
          <i style={{ transform: `scaleX(${Math.min(1, leftMs / total)})` }} />
        </div>
        <p className="countdown-text" role="timer">
          {t.countdown(seconds)}
        </p>
        <div className="approval-actions">
          <button ref={denyRef} type="button" className="btn large" disabled={sending} onClick={() => answer(false)}>
            {t.deny}
          </button>
          <button type="button" className={`btn large${words.unknown ? "" : " primary"}`} disabled={sending} onClick={() => answer(true)}>
            {t.allow}
          </button>
        </div>
        <div className="approval-pause">
          <button type="button" className="link" onClick={() => void act(() => backend.pause())}>
            {t.pauseAll}
          </button>
        </div>
      </div>
    </div>
  );
}
