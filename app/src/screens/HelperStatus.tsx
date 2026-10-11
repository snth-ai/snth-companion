// What computer control needs on this computer: the helper app and, on
// macOS, two permissions. Used in onboarding (checking live) and on the
// Computer control page.

import { useCallback, useEffect, useState } from "react";
import { backend } from "../backend";
import type { CuaStatus } from "../backend/types";
import { useStore } from "../lib/store";
import { More } from "../ui/controls";
import { IconCheck, IconWarning } from "../ui/icons";

function PermState(props: { value: boolean | null; yes: string; no: string; unknown: string }) {
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
  return <span className="perm-state">{props.unknown}</span>;
}

export function useHelperStatus(live: boolean) {
  const [cua, setCua] = useState<CuaStatus | null>(null);
  const [checking, setChecking] = useState(false);

  const check = useCallback(async () => {
    setChecking(true);
    try {
      setCua(await backend.cuaCheck());
    } catch {
      /* keep the last answer */
    } finally {
      setChecking(false);
    }
  }, []);

  useEffect(() => {
    void check();
    if (!live) return;
    const id = window.setInterval(() => void check(), 3000);
    return () => window.clearInterval(id);
  }, [check, live]);

  return { cua, checking, check };
}

/** Everything computer control needs is in place. */
export function helperReady(cua: CuaStatus | null, os: string): boolean {
  if (!cua?.installed) return false;
  if (os !== "macos") return true;
  return cua.accessibility === true && cua.screen_recording === true;
}

export function HelperStatus(props: { live: boolean; notReadyNote?: string }) {
  const { state, s } = useStore();
  const t = s.computer;
  const os = state.app.os;
  const mac = os === "macos";
  // After "ask for permissions" keep checking for a while even on the
  // settings page, so the answer shows up without a click.
  const [watch, setWatch] = useState(false);
  const { cua, checking, check } = useHelperStatus(props.live || watch);
  const [started, setStarted] = useState(false);

  useEffect(() => {
    if (!watch) return;
    const id = window.setTimeout(() => setWatch(false), 60_000);
    return () => window.clearTimeout(id);
  }, [watch]);

  const old = cua?.installed && cua.version !== null && cua.version !== cua.pinned_version;
  const needsGrant = mac && cua?.installed && !(cua.accessibility === true && cua.screen_recording === true);

  return (
    <>
      <div className="group">
        <div className="row">
          <div className="row-text">
            <div className="row-title">{t.helper}</div>
            {started ? <div className="row-desc">{t.helperStarted}</div> : null}
          </div>
          {cua === null ? (
            <span className="perm-state">{t.checking}</span>
          ) : !cua.installed ? (
            <>
              <PermState value={false} yes="" no={t.helperMissing} unknown="" />
              <button type="button" className="btn small" onClick={() => void backend.openExternal("helper")}>
                {t.helperGet}
              </button>
            </>
          ) : old ? (
            <>
              <PermState value={false} yes="" no={t.helperOld} unknown="" />
              <button
                type="button"
                className="btn small"
                onClick={() => {
                  setStarted(true);
                  setWatch(true);
                  void backend.cuaInstall().catch(() => setStarted(false));
                }}
              >
                {t.helperUpdate}
              </button>
            </>
          ) : (
            <PermState value yes={t.helperInstalled} no="" unknown="" />
          )}
        </div>

        {mac ? (
          <>
            <div className="row">
              <div className="row-text">
                <div className="row-title">{t.permControl}</div>
                <div className="row-desc">{t.permControlSys}</div>
              </div>
              <PermState value={cua?.installed ? cua.accessibility : null} yes={t.granted} no={t.notGranted} unknown={cua?.installed ? t.unknown : ""} />
              {cua?.installed && cua.accessibility === false ? (
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
              <PermState value={cua?.installed ? cua.screen_recording : null} yes={t.granted} no={t.notGranted} unknown={cua?.installed ? t.unknown : ""} />
              {cua?.installed && cua.screen_recording === false ? (
                <button type="button" className="btn small" onClick={() => void backend.openExternal("privacy_screen")}>
                  {t.openSettings}
                </button>
              ) : null}
            </div>
          </>
        ) : null}
      </div>

      {!mac ? <p className="section-note">{os === "linux" ? t.waylandNote : t.noPermsNeeded}</p> : null}

      <div className="onb-actions">
        {needsGrant ? (
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
        ) : null}
        <button type="button" className="btn" disabled={checking} onClick={() => void check()}>
          {t.checkAgain}
        </button>
        {needsGrant ? <span className="muted" style={{ fontSize: 12, flex: "1 1 200px" }}>{t.grantHint}</span> : null}
      </div>

      {props.notReadyNote && cua !== null && !helperReady(cua, os) ? <p className="section-note">{props.notReadyNote}</p> : null}

      <More label={s.common.details}>
        <div>{t.detailsHelper(cua?.version ?? "-", cua?.pinned_version ?? state.app.cua_pinned_version)}</div>
        <div>{t.detailsPath(state.app.cua_path)}</div>
        {cua?.error ? <div>{t.detailsError(cua.error)}</div> : null}
      </More>
    </>
  );
}
