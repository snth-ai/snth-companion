// What computer control needs on this computer: the helper app, which
// SNTH Connect installs by itself, and on macOS two permissions the
// system gives to SNTH Connect. Used in onboarding and on the Computer
// control page.
//
// The two are independent: the permissions belong to this app, so the
// person can grant them while the helper is still downloading.

import { useCallback, useEffect, useState } from "react";
import { backend } from "../backend";
import type { CuaStatus, HelperState } from "../backend/types";
import { formatClock } from "../lib/format";
import { useStore } from "../lib/store";
import { More } from "../ui/controls";
import { IconCheck, IconWarning } from "../ui/icons";

function PermState(props: { value: boolean | null; yes: string; no: string }) {
  if (props.value === true) {
    return (
      <span className="perm-state yes">
        <IconCheck />
        {props.yes}
      </span>
    );
  }
  if (props.value === false) {
    return (
      <span className="perm-state no">
        <IconWarning />
        {props.no}
      </span>
    );
  }
  return null;
}

/** The helper state and the permissions, kept current: helper progress
 * arrives as events, permissions are re-read on a timer while `live`. */
export function useHelperStatus(live: boolean) {
  const { state } = useStore();
  const [cua, setCua] = useState<CuaStatus | null>(null);
  const [checking, setChecking] = useState(false);
  // Progress events are newer than the last full check.
  const [pushed, setPushed] = useState<HelperState | null>(null);

  const check = useCallback(async () => {
    setChecking(true);
    try {
      setCua(await backend.cuaCheck());
      setPushed(null);
    } catch {
      /* keep the last answer */
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    void check();
    const stop = backend.subscribe((event) => {
      if (event.type === "helper") setPushed(event.state);
      // A settled install (ready, failed, a new retry time) is in the state.
      else if (event.type === "state") void check();
    });
    const id = live ? window.setInterval(() => void check(), 3000) : undefined;
    return () => {
      stop();
      if (id !== undefined) window.clearInterval(id);
    };
  }, [check, live]);

  const helper: HelperState = pushed ?? cua?.helper ?? state.helper;
  return { cua, helper, retryAt: cua?.retry_at ?? state.helper_retry_at, checking, check };
}

export function HelperStatus(props: { live: boolean; notReadyNote?: string }) {
  const { state, s, lang } = useStore();
  const t = s.computer;
  const os = state.app.os;
  const mac = os === "macos";
  // After "ask for permissions" keep checking for a while even on the
  // settings page, so the answer shows up without a click.
  const [watch, setWatch] = useState(false);
  const { cua, helper, retryAt, checking, check } = useHelperStatus(props.live || watch);

  useEffect(() => {
    if (!watch) return;
    const id = window.setTimeout(() => setWatch(false), 60_000);
    return () => window.clearTimeout(id);
  }, [watch]);

  const percent =
    helper.state === "downloading" && helper.total ? Math.min(100, Math.round((helper.received / helper.total) * 100)) : null;
  const busy = helper.state !== "ready" && helper.state !== "failed";
  const needsGrant = mac && cua !== null && !(cua.accessibility === true && cua.screen_recording === true);
  const canRetry = helper.state === "failed" && helper.code !== "unsupported";

  return (
    <>
      <div className="group">
        <div className="row" aria-live="polite">
          <div className="row-text">
            <div className="row-title">{t.helper}</div>
            <div className="row-desc">
              {helper.state === "failed" ? (
                <>
                  {t.helperWhy[helper.code] ?? t.helperWhy.download}
                  {canRetry && retryAt ? ` ${t.helperRetryAt(formatClock(retryAt, lang))}` : ""}
                </>
              ) : busy ? (
                t.helperWorking
              ) : (
                t.helperDesc
              )}
            </div>
            {helper.state === "downloading" ? (
              <div className="meter" aria-hidden="true">
                <i style={{ width: `${percent ?? 4}%` }} />
              </div>
            ) : null}
          </div>
          {helper.state === "ready" ? (
            <PermState value yes={t.helperReady} no="" />
          ) : helper.state === "failed" ? (
            <>
              <PermState value={false} yes="" no={t.helperFailed} />
              {canRetry ? (
                <button type="button" className="btn small" onClick={() => void backend.cuaInstall().then(check)}>
                  {t.helperRetry}
                </button>
              ) : null}
            </>
          ) : (
            <span className="perm-state">
              {helper.state === "downloading"
                ? t.helperDownloading(percent)
                : helper.state === "installing" || helper.state === "verifying"
                  ? t.helperInstalling
                  : t.helperChecking}
            </span>
          )}
        </div>

        {mac ? (
          <>
            <div className="row">
              <div className="row-text">
                <div className="row-title">{t.permControl}</div>
                <div className="row-desc">{t.permControlSys}</div>
              </div>
              <PermState value={cua?.accessibility ?? null} yes={t.granted} no={t.notGranted} />
              {cua?.accessibility === false ? (
                <button type="button" className="btn small" onClick={() => void backend.openExternal("privacy_accessibility")}>
                  {t.openSettings}
                </button>
              ) : null}
            </div>
            <div className="row">
              <div className="row-text">
                <div className="row-title">{t.permSee}</div>
                <div className="row-desc">{t.permSeeSys}</div>
              </div>
              <PermState value={cua?.screen_recording ?? null} yes={t.granted} no={t.notGranted} />
              {cua?.screen_recording === false ? (
                <button type="button" className="btn small" onClick={() => void backend.openExternal("privacy_screen")}>
                  {t.openSettings}
                </button>
              ) : null}
            </div>
          </>
        ) : null}
      </div>

      {!mac ? <p className="section-note">{os === "linux" ? t.waylandNote : t.noPermsNeeded}</p> : null}

      {needsGrant ? (
        <>
          <div className="onb-actions">
            <button
              type="button"
              className="btn primary"
              onClick={() => {
                setWatch(true);
                void backend.cuaGrantPermissions().catch(() => undefined);
              }}
            >
              {t.grant}
            </button>
            <button type="button" className="btn" disabled={checking} onClick={() => void check()}>
              {t.checkAgain}
            </button>
            <span className="muted" style={{ fontSize: 12, flex: "1 1 200px" }}>
              {t.grantHint}
            </span>
          </div>
          <p className="section-note">{t.relaunchNote}</p>
        </>
      ) : null}

      {/* Something is left for the person to do (a permission, or the
          install failed): say where to finish it. A running install needs
          nothing from them. */}
      {props.notReadyNote && (needsGrant || helper.state === "failed") ? (
        <p className="section-note">{props.notReadyNote}</p>
      ) : null}

      <More label={s.common.details}>
        <div>{t.detailsHelper(helper.state === "ready" ? helper.version : "-", cua?.pinned_version ?? state.app.cua_pinned_version)}</div>
        <div>{t.detailsPath(state.app.cua_path)}</div>
        {helper.state === "failed" && helper.detail ? <div>{t.detailsError(helper.detail)}</div> : null}
        <div style={{ marginTop: 4 }}>
          <button type="button" className="link" onClick={() => void backend.openExternal("helper")}>
            {t.detailsSite}
          </button>
        </div>
      </More>
    </>
  );
}
