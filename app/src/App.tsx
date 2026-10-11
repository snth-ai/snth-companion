import { useEffect, useRef, useState, type ReactNode } from "react";
import { backend } from "./backend";
import { shortcutText } from "./lib/format";
import { useActiveEntry, useStore } from "./lib/store";
import { Activity } from "./screens/Activity";
import { Computer } from "./screens/Computer";
import { Cookies } from "./screens/Cookies";
import { Files } from "./screens/Files";
import { General } from "./screens/General";
import { Internet } from "./screens/Internet";
import { Onboarding, type Step } from "./screens/Onboarding";
import { Status } from "./screens/Status";
import { Sheet } from "./ui/controls";
import { IconCookie, IconFolder, IconGlobe, IconList, IconPause, IconPlay, IconPointer, IconSliders, IconStatus } from "./ui/icons";

export type Screen = "status" | "internet" | "computer" | "files" | "cookies" | "activity" | "general";
const SCREENS: Screen[] = ["status", "internet", "cookies", "computer", "files", "activity", "general"];

function Sidebar(props: { screen: Screen; go: (screen: Screen) => void }) {
  const { state, s, name, act } = useStore();
  const core = state.core;
  const settings = core.settings;
  const active = useActiveEntry();

  const link = core.paused
    ? { dot: "warn", text: s.link.paused }
    : state.needs_repair
      ? { dot: "danger", text: s.link.repair }
      : core.link_state === "linked"
        ? active
          ? { dot: "busy", text: s.link.working }
          : { dot: "ok", text: s.link.linked }
        : core.link_state === "connecting"
          ? { dot: "", text: s.link.connecting }
          : { dot: "", text: s.link.reconnecting };

  const items: { screen: Screen; icon: ReactNode; title: string; off?: boolean }[] = [
    { screen: "status", icon: <IconStatus />, title: s.nav.status },
    { screen: "internet", icon: <IconGlobe />, title: s.nav.internet, off: !settings.net.enabled },
    { screen: "cookies", icon: <IconCookie />, title: s.nav.cookies, off: !settings.cookies.enabled },
    { screen: "computer", icon: <IconPointer />, title: s.nav.computer, off: !settings.computer.enabled },
    { screen: "files", icon: <IconFolder />, title: s.nav.files, off: !settings.files.enabled },
    { screen: "activity", icon: <IconList />, title: s.nav.activity },
    { screen: "general", icon: <IconSliders />, title: s.nav.general },
  ];

  const shortcut = state.prefs.stop_shortcut && state.prefs.stop_shortcut_active ? shortcutText(state.prefs.stop_shortcut, state.app.os) : "";

  return (
    <aside className={`side${core.paused ? " paused" : ""}`}>
      <div className="who">
        <div className="who-name" title={name}>
          {name}
        </div>
        <div className="who-state" role="status">
          <span className={`dot ${link.dot}`} />
          {link.text}
        </div>
      </div>

      <nav className="nav" aria-label={s.appName}>
        {items.map((item) => (
          <button
            key={item.screen}
            type="button"
            className="nav-item"
            aria-current={props.screen === item.screen ? "page" : undefined}
            onClick={() => props.go(item.screen)}
          >
            {item.icon}
            {item.title}
            {item.off ? <span className="nav-flag">{s.common.off}</span> : null}
          </button>
        ))}
      </nav>

      <div className="side-foot">
        {core.paused ? (
          <button type="button" className="btn strong large" onClick={() => void act(() => backend.resume())}>
            <IconPlay />
            {s.side.resume}
          </button>
        ) : (
          <button type="button" className="btn large" title={s.side.pauseTitle(name)} onClick={() => void act(() => backend.pause())}>
            <IconPause />
            {s.side.pause}
          </button>
        )}
        {shortcut && !core.paused ? <div className="side-shortcut">{s.side.shortcut(shortcut)}</div> : null}
      </div>
    </aside>
  );
}

export function App(props: { initialScreen?: string | null; initialStep?: Step }) {
  const { state, s, name, failed, linkCode, takeLinkCode } = useStore();
  const [screen, setScreen] = useState<Screen>(() =>
    SCREENS.includes(props.initialScreen as Screen) ? (props.initialScreen as Screen) : "status",
  );
  const mainRef = useRef<HTMLElement>(null);
  const linked = state.core.paired;
  const ready = linked && state.prefs.onboarded;

  // After unlinking and linking again, start on Status.
  useEffect(() => {
    if (!ready) setScreen("status");
  }, [ready]);

  const go = (next: Screen) => {
    setScreen(next);
    mainRef.current?.scrollTo({ top: 0 });
  };

  // A pairing link while this computer is already linked: say so, change
  // nothing. (While not linked, onboarding picks the code up.)
  const strayLink = linked && linkCode !== null;

  if (!ready) {
    return (
      <>
        <Onboarding initialStep={props.initialStep} />
        {strayLink ? <AlreadyLinked name={name} onClose={takeLinkCode} /> : null}
      </>
    );
  }

  return (
    <div className="app">
      <Sidebar screen={screen} go={go} />
      <main className="main" ref={mainRef}>
        {failed ? (
          <p className="error-text" role="alert" style={{ margin: "0 0 10px" }}>
            {s.common.failed}
          </p>
        ) : null}
        {screen === "status" ? <Status go={go} /> : null}
        {screen === "internet" ? <Internet /> : null}
        {screen === "cookies" ? <Cookies /> : null}
        {screen === "computer" ? <Computer /> : null}
        {screen === "files" ? <Files /> : null}
        {screen === "activity" ? <Activity /> : null}
        {screen === "general" ? <General /> : null}
      </main>
      {strayLink ? <AlreadyLinked name={name} onClose={takeLinkCode} /> : null}
    </div>
  );
}

function AlreadyLinked(props: { name: string; onClose: () => void }) {
  const { s } = useStore();
  return (
    <Sheet
      title={s.onboarding.alreadyLinkedTitle}
      onCancel={props.onClose}
      actions={
        <button type="button" className="btn primary" data-autofocus onClick={props.onClose}>
          {s.common.close}
        </button>
      }
    >
      <p style={{ marginTop: 10, lineHeight: 1.45 }}>{s.onboarding.alreadyLinkedBody(props.name)}</p>
    </Sheet>
  );
}
