// The request window. The app's own wording comes first and says what
// would happen; the synth's reason is shown after it, marked as the
// synth's words. "Don't allow" has the focus, a closed window is a
// refusal, and so is no answer within the countdown.

import { useEffect, useRef, useState } from "react";
import { backend } from "../backend";
import type { AnswerDuration, ApprovalRequest } from "../backend/types";
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
  /** A request this version does not understand: Allow is not offered
   * as the obvious button. */
  unknown?: boolean;
}

function wording(req: ApprovalRequest, s: Strings, name: string, cookiesAskEveryTime: boolean): Wording {
  const t = s.approval;
  switch (req.capability) {
    case "net":
      // One page is many connections, so "this time only" would ask again
      // at once; the shortest useful answer is an hour.
      return { title: t.netTitle(req.scope), body: t.netBody, spans: ["hour", "hours8", "session", "always"] };
    case "cookies":
      return cookiesAskEveryTime
        ? { title: t.cookiesTitle(req.scope), body: t.cookiesBody(name), spans: ["once"], hint: t.onceOnly }
        : { title: t.cookiesTitle(req.scope), body: t.cookiesBody(name), spans: ["once", "session", "always"] };
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
  }
  // Something this version does not know: never remembered.
  return { title: t.otherTitle, body: t.otherBody(req.capability, req.scope), spans: ["once"], unknown: true };
}

export function ApprovalView() {
  const { state, s, name, act } = useStore();
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
          {name.charAt(0).toUpperCase()}
        </span>
        <span className="approval-who">{t.asks(name)}</span>
        {live.length > 1 ? <span className="approval-queue">{t.queue(1, live.length)}</span> : null}
      </div>

      <div className="approval-body">
        <h1>{words.title}</h1>
        <p className="approval-what">{words.body}</p>

        <div className="says">
          <div className="says-label">{t.says(name)}</div>
          <div className="says-text selectable">{current.reason ? `"${current.reason}"` : t.noReason}</div>
          {current.reason ? <div className="says-note">{t.unverified}</div> : null}
        </div>

        {words.spans.length > 1 ? (
          <div className="howlong">
            <div className="howlong-label">{t.howLong}</div>
            <Segmented<Span>
              label={t.howLong}
              value={chosen}
              onChange={setSpan}
              options={words.spans.map((v) => ({ value: v, title: t[v] }))}
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
