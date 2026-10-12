import { useMemo, type ReactNode } from "react";
import { backend } from "../backend";
import { describeEntry, formatAgo, formatBytes, summarize, todayCounters } from "../lib/format";
import { useActiveEntry, useNow, useStore } from "../lib/store";
import { Notice, Section } from "../ui/controls";
import { IconChevron, IconCookie, IconFolder, IconGlobe, IconPause, IconPlay, IconPointer, IconBlock } from "../ui/icons";
import type { Screen } from "../App";
import { UpdateBlockedNotice } from "./General";

export function Status(props: { go: (screen: Screen) => void }) {
  const { state, journal, s, lang, name, act } = useStore();
  const core = state.core;
  const settings = core.settings;
  const active = useActiveEntry();
  useNow(30_000); // keeps "5 minutes ago" honest

  const today = useMemo(() => todayCounters(journal), [journal]);
  const last = journal[journal.length - 1];

  const linkLine =
    core.link_state === "linked" || core.paused
      ? s.status.onThisComputer(state.app.device_name)
      : core.link_state === "connecting"
        ? s.status.connecting(name)
        : s.status.reconnecting(name);

  const rows: { screen: Screen; icon: ReactNode; title: string; on: boolean; cap: "net" | "cookies" | "computer" | "files" }[] = [
    { screen: "internet", icon: <IconGlobe />, title: s.nav.internet, on: settings.net.enabled, cap: "net" },
    { screen: "cookies", icon: <IconCookie />, title: s.nav.cookies, on: settings.cookies.enabled, cap: "cookies" },
    { screen: "computer", icon: <IconPointer />, title: s.nav.computer, on: settings.computer.enabled, cap: "computer" },
    { screen: "files", icon: <IconFolder />, title: s.nav.files, on: settings.files.enabled, cap: "files" },
  ];

  const todayParts: ReactNode[] = [];
  if (today.netCount > 0) {
    todayParts.push(
      <span key="net">
        <IconGlobe />
        {s.status.todayNet(today.netCount, formatBytes(today.netBytes, s, lang))}
      </span>,
    );
  }
  if (today.cookies > 0) {
    todayParts.push(
      <span key="cookies">
        <IconCookie />
        {s.status.todayCookies(today.cookies)}
      </span>,
    );
  }
  if (today.computer > 0) {
    todayParts.push(
      <span key="computer">
        <IconPointer />
        {s.status.todayComputer(today.computer)}
      </span>,
    );
  }
  if (today.files > 0) {
    todayParts.push(
      <span key="files">
        <IconFolder />
        {s.status.todayFiles(today.files)}
      </span>,
    );
  }
  if (today.refused > 0) {
    todayParts.push(
      <span key="refused" className="refused">
        <IconBlock />
        {s.status.todayRefused(today.refused)}
      </span>,
    );
  }

  return (
    <>
      <h1 className="page-title">{name}</h1>
      <p className="page-lead">{linkLine}</p>

      {/* macOS: running from the download or a disk image. Worth saying on
          the first screen: the app should be moved before anything else. */}
      {state.update.configured && (state.update.blocked === "translocated" || state.update.blocked === "read_only") ? (
        <div style={{ marginTop: 16 }}>
          <UpdateBlockedNotice />
        </div>
      ) : null}

      {state.needs_repair ? (
        <div style={{ marginTop: 16 }}>
          <Notice
            kind="danger"
            title={s.status.repairTitle}
            actions={
              <button type="button" className="btn danger" onClick={() => props.go("general")}>
                {s.status.repairAction}
              </button>
            }
          >
            {s.status.repairBody(name)}
          </Notice>
        </div>
      ) : null}

      {core.paused ? (
        <div className="paused-block" role="status">
          <h2>
            <IconPause />
            {s.status.pausedTitle}
          </h2>
          <p>{s.status.pausedBody(name)}</p>
          <button type="button" className="btn strong large" onClick={() => void act(() => backend.resume())}>
            <IconPlay />
            {s.status.resume}
          </button>
        </div>
      ) : (
        <div className={`now${active ? "" : " idle"}`} aria-live="polite">
          <span className={`dot${active ? " busy" : ""}`} />
          <div className="now-text">
            <div className="now-label">
              <span>{s.status.rightNow}</span>
              {!active && last ? <span>{s.status.lastActivity(formatAgo(last.time, lang))}</span> : null}
            </div>
            <div className="now-line">
              {active ? describeEntry(active, s, lang).text : last ? s.status.nothingNow : s.status.neverActive(name)}
            </div>
          </div>
        </div>
      )}

      <Section title={s.status.allowedTitle(name)}>
        <div className="group">
          {rows.map((r) => (
            <button type="button" className="row" key={r.screen} onClick={() => props.go(r.screen)}>
              <span className={`row-icon${r.on ? " on" : ""}`}>{r.icon}</span>
              <span className="row-text">
                <span className="row-title">{r.title}</span>
              </span>
              <span className="row-value">
                {r.cap === "computer" && r.on && state.helper.state !== "ready"
                  ? state.helper.state === "failed"
                    ? s.status.sum.helperMissing
                    : s.status.sum.helperInstalling
                  : summarize(r.cap, settings, s)}
              </span>
              <IconChevron />
            </button>
          ))}
        </div>
      </Section>

      <Section title={s.status.today}>
        <div className="group">
          {todayParts.length > 0 ? <div className="today">{todayParts}</div> : <div className="empty">{s.status.todayNothing}</div>}
        </div>
      </Section>

    </>
  );
}
