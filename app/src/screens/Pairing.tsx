// A code from the bot, once the app is past onboarding: add another synth,
// or give a synth that no longer knows this computer a new key. Claiming
// the code only shows who it belongs to; nothing is linked before the
// Connect click, and nothing of the settings is reset (that happens only
// for the first link, in onboarding).

import { useEffect, useRef, useState, type RefObject } from "react";
import { backend } from "../backend";
import type { Pending } from "../backend/types";
import { hostOf } from "../lib/format";
import { useStore } from "../lib/store";
import { Notice, Sheet } from "../ui/controls";

/** The 6-digit field, as onboarding shows it. */
export function CodeInput(props: {
  value: string;
  onChange: (next: string) => void;
  onSubmit: () => void;
  busy: boolean;
  error: string | null;
  inputRef?: RefObject<HTMLInputElement | null>;
}) {
  const { s } = useStore();
  return (
    <>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (props.value.length === 6 && !props.busy) props.onSubmit();
        }}
      >
        <input
          ref={props.inputRef}
          className="code-input"
          inputMode="numeric"
          autoComplete="one-time-code"
          aria-label={s.onboarding.codeLabel}
          aria-invalid={props.error !== null}
          placeholder="000000"
          value={props.value}
          disabled={props.busy}
          onChange={(e) => props.onChange(e.target.value.replace(/\D/g, "").slice(0, 6))}
        />
      </form>
      {props.error ? (
        <p className="error-text" role="alert" style={{ fontSize: 13 }}>
          {props.error}
        </p>
      ) : null}
    </>
  );
}

/** What the platform said about the synth behind a code (signed in only). */
export function AccountNote(props: { pending: Pending }) {
  const { s } = useStore();
  const n = props.pending.synth_name || props.pending.synth_id;
  if (props.pending.linked_to_account === false && props.pending.reason === "synth_linked_elsewhere") {
    return <Notice>{s.onboarding.linkedElsewhere(n)}</Notice>;
  }
  if (props.pending.linked_to_account === true) return <Notice>{s.onboarding.addedToAccount(n)}</Notice>;
  return null;
}

/** The error code of a failed claim, in the owner's words. */
export function claimError(e: unknown, errors: Record<string, string>): string {
  const key = String(e).split(":")[0]!.trim();
  return errors[key] ?? errors.hub_error!;
}

/** Enter a code (or take one from a link), see who it belongs to, connect. */
export function PairSheet(props: { title: string; body: string; initialCode?: string; fromLink?: boolean; onClose: () => void }) {
  const { state, s, act } = useStore();
  const t = s.onboarding;
  const [code, setCode] = useState(props.initialCode ?? "");
  const [pending, setPending] = useState<Pending | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const claim = async (value: string) => {
    setBusy(true);
    setError(null);
    try {
      setPending(await backend.pairBegin(value));
    } catch (e) {
      setError(claimError(e, t.errors));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    if (props.initialCode) void claim(props.initialCode);
    else inputRef.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const cancel = () => {
    if (busy) return;
    if (pending) void backend.pairCancel();
    props.onClose();
  };

  if (pending) {
    const n = pending.synth_name || pending.synth_id;
    return (
      <Sheet
        title={t.confirmTitle(n)}
        onCancel={cancel}
        actions={
          <>
            <button type="button" className="btn" disabled={busy} onClick={cancel}>
              {s.common.cancel}
            </button>
            <button
              type="button"
              className="btn primary"
              data-autofocus
              disabled={busy}
              onClick={() => {
                setBusy(true);
                void act(() => backend.pairConfirm()).then((ok) => {
                  setBusy(false);
                  if (ok) props.onClose();
                });
              }}
            >
              {busy ? t.connecting : t.connect}
            </button>
          </>
        }
      >
        <div className="group facts">
          <div className="row">
            <span className="row-label">{t.confirmSynth}</span>
            <span className="row-text selectable" style={{ fontWeight: 600 }}>
              {n}
            </span>
          </div>
          <div className="row">
            <span className="row-label">{t.confirmAddress}</span>
            <span className="row-text selectable">{hostOf(pending.synth_url)}</span>
          </div>
          <div className="row">
            <span className="row-label">{t.confirmComputer}</span>
            <span className="row-text">{state.app.device_name}</span>
          </div>
        </div>
        <div style={{ marginTop: 12, display: "grid", gap: 8 }}>
          <AccountNote pending={pending} />
          {props.fromLink ? <Notice kind="warn">{t.confirmFromLink}</Notice> : null}
        </div>
      </Sheet>
    );
  }

  return (
    <Sheet
      title={props.title}
      onCancel={cancel}
      actions={
        <>
          <button type="button" className="btn" onClick={cancel}>
            {s.common.cancel}
          </button>
          <button type="button" className="btn primary" disabled={busy || code.length !== 6} onClick={() => void claim(code)}>
            {busy ? t.checking : s.common.continue}
          </button>
        </>
      }
    >
      <p style={{ marginTop: 10, lineHeight: 1.45 }}>{props.body}</p>
      <CodeInput
        value={code}
        onChange={(next) => {
          setCode(next);
          setError(null);
        }}
        onSubmit={() => void claim(code)}
        busy={busy}
        error={error}
        inputRef={inputRef}
      />
    </Sheet>
  );
}
