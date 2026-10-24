import { useEffect, useRef, useState, type ReactNode } from "react";
import { backend } from "./backend";
import { shortcutText } from "./lib/format";
import { synthName, synthsLabel, synthsNames, useActiveEntry, useStore } from "./lib/store";
import { Activity } from "./screens/Activity";
import { Apps } from "./screens/Apps";
import { Computer } from "./screens/Computer";
import { Files } from "./screens/Files";
import { General } from "./screens/General";
import { Internet } from "./screens/Internet";
import { Island } from "./screens/Island";
import { Logins } from "./screens/Logins";
import { Observe } from "./screens/Observe";
import { Onboarding, type Step } from "./screens/Onboarding";
import { PairSheet } from "./screens/Pairing";
import { Status } from "./screens/Status";
import { IconApps, IconCookie, IconEye, IconIsland, IconFolder, IconGlobe, IconList, IconPause, IconPlay, IconPointer, IconSliders, IconStatus } from "./ui/icons";

export type Screen = "status" | "internet" | "computer" | "files" | "apps" | "island" | "observe" | "cookies" | "activity" | "general";
const SCREENS: Screen[] = ["status", "internet", "cookies", "computer", "files", "apps", "island", "observe", "activity", "general"];

function Sidebar(props: { screen: Screen; go: (screen: Screen) => void }) {
  const { state, s, act } = useStore();
  const core = state.core;
  const settings = core.settings;
  const active = useActiveEntry();
  // With several synths the line says how the links stand together: one
  // that needs relinking is worth a red dot even while the others work.
  const someRepair = core.links.some((l) => l.state === "needs_repair");

  const link = core.paused
    ? { dot: "warn", text: s.link.paused }
    : state.needs_repair
      ? { dot: "danger", text: s.link.repair }
      : core.link_state === "linked"
        ? active
          ? { dot: "busy", text: s.link.working }
          : someRepair
            ? { dot: "danger", text: s.link.repair }
            : { dot: "ok", text: s.link.linked }
        : core.link_state === "connecting"
          ? { dot: "", text: s.link.connecting }
          : { dot: "", text: s.link.reconnecting };
  const who = synthsLabel(state, s);

  const items: { screen: Screen; icon: ReactNode; title: string; off?: boolean }[] = [
    { screen: "status", icon: <IconStatus />, title: s.nav.status },
    { screen: "internet", icon: <IconGlobe />, title: s.nav.internet, off: !settings.net.enabled },
    { screen: "cookies", icon: <IconCookie />, title: s.nav.cookies, off: !settings.cookies.enabled },
    { screen: "computer", icon: <IconPointer />, title: s.nav.computer, off: !settings.computer.enabled },
    { screen: "files", icon: <IconFolder />, title: s.nav.files, off: !settings.files.enabled },
    { screen: "apps", icon: <IconApps />, title: s.nav.apps, off: !settings.apps.enabled },
    { screen: "island", icon: <IconIsland />, title: s.nav.island, off: !state.island.settings.enabled },
    {
      screen: "observe",
      icon: <IconEye />,
      title: s.nav.observe(core.links.length > 1 ? null : synthName(state, s)),
      off: !(state.observe?.links ?? []).some((l) => l.on.length > 0),
    },
    { screen: "activity", icon: <IconList />, title: s.nav.activity },
    { screen: "general", icon: <IconSliders />, title: s.nav.general },
  ];

  const shortcut = state.prefs.stop_shortcut && state.prefs.stop_shortcut_active ? shortcutText(state.prefs.stop_shortcut, state.app.os) : "";

  return (
    <aside className={`side${core.paused ? " paused" : ""}`}>
      <div className="who">
        <div className="who-name" title={who}>
          {who}
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
          <button type="button" className="btn large" title={state.core.links.length > 1 ? s.side.pauseTitleMany : s.side.pauseTitle(synthsNames(state, s))} onClick={() => void act(() => backend.pause())}>
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
  const { state, s, failed, linkCode, takeLinkCode } = useStore();
  const [screen, setScreen] = useState<Screen>(() => {
    // "logins" is the page's name; "cookies" is what it was called before
    // and what the code still calls the capability.
    const asked = props.initialScreen === "logins" ? "cookies" : props.initialScreen;
    return SCREENS.includes(asked as Screen) ? (asked as Screen) : "status";
  });
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

  // The island's "Open SNTH Connect" brings the window up at a screen.
  useEffect(
    () =>
      backend.subscribe((event) => {
        if (event.type === "open_screen" && SCREENS.includes(event.screen as Screen)) {
          setScreen(event.screen as Screen);
          mainRef.current?.scrollTo({ top: 0 });
        }
      }),
    [],
  );

  // A pairing link while this computer is already linked: it adds another
  // synth (or gives one a new key). The sheet shows who the code belongs
  // to; nothing changes before Connect, and the settings stay. (While not
  // linked, onboarding picks the code up.)
  const [linkSheet, setLinkSheet] = useState<string | null>(null);
  useEffect(() => {
    if (linked && linkCode !== null) {
      setLinkSheet(linkCode);
      takeLinkCode();
    }
  }, [linked, linkCode, takeLinkCode]);
  const strayLink = linkSheet !== null ? (
    <PairSheet
      title={s.account.addTitle}
      body={s.onboarding.codeBody}
      initialCode={linkSheet}
      fromLink
      onClose={() => setLinkSheet(null)}
    />
  ) : null;

  if (!ready) {
    return (
      <>
        <Onboarding initialStep={props.initialStep} />
        {strayLink}
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
        {screen === "cookies" ? <Logins /> : null}
        {screen === "computer" ? <Computer /> : null}
        {screen === "files" ? <Files /> : null}
        {screen === "apps" ? <Apps /> : null}
        {screen === "island" ? <Island /> : null}
        {screen === "observe" ? <Observe /> : null}
        {screen === "activity" ? <Activity /> : null}
        {screen === "general" ? <General /> : null}
      </main>
      {strayLink}
    </div>
  );
}
