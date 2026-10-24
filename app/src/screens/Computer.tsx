import { useEffect, useMemo, useState } from "react";
import { backend } from "../backend";
import type { AppChoice, AppRef, AppsPolicy, ComputerMode, CursorColor, CursorSpeed, Experiment, Foreground, ReducedMotion } from "../backend/types";
import { useStore } from "../lib/store";
import { ChoiceList, Notice, Row, Section, Segmented, Sheet, SwitchRow } from "../ui/controls";
import { IconPlus, IconX } from "../ui/icons";
import { HelperStatus } from "./HelperStatus";
import { Remembered } from "./Remembered";

const AWAY_MINUTES = [1, 2, 5, 10, 15, 30, 60];

/** The fixed colours of the pointer, as the app's cursor themes have them
 * (`crates/connect-core/src/cua_look.rs`, `THEMES`). */
const COLORS: { value: Exclude<CursorColor, "auto">; hex: string }[] = [
  { value: "sky", hex: "#5EC0E8" },
  { value: "violet", hex: "#B284FF" },
  { value: "rose", hex: "#F0569A" },
  { value: "green", hex: "#3FCF8E" },
  { value: "amber", hex: "#F4B242" },
  { value: "red", hex: "#E85262" },
  { value: "graphite", hex: "#4B5160" },
];
/** A few of the colours the helper itself gives to tasks. */
const AUTO_COLORS = ["#B284FF", "#60DAAE", "#F4B242"];
const IDLE_HIDE = [5, 15, 60, 0];
const SCREENSHOTS = [0, 1024, 1568, 2048];

/** The helper's pointer, in one colour. */
function Pointer(props: { color: string }) {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" aria-hidden="true">
      <path
        d="M5.6 4.3 19 9.6c.9.4.9 1.6 0 2l-5 2.1-2.1 5c-.4.9-1.6.9-2 0L4.3 5.6c-.4-.9.4-1.7 1.3-1.3Z"
        fill={props.color}
        stroke="#fff"
        strokeWidth="1.8"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** One pointer colour among a few, each shown as the pointer itself. */
function ColorChoice(props: { label: string; value: CursorColor; names: Record<CursorColor, string>; onChange: (next: CursorColor) => void }) {
  return (
    <div className="swatches" role="radiogroup" aria-label={props.label}>
      <button
        type="button"
        role="radio"
        className="swatch auto"
        aria-checked={props.value === "auto"}
        aria-label={props.names.auto}
        title={props.names.auto}
        onClick={() => props.onChange("auto")}
      >
        {AUTO_COLORS.map((hex) => (
          <Pointer key={hex} color={hex} />
        ))}
      </button>
      {COLORS.map((c) => (
        <button
          key={c.value}
          type="button"
          role="radio"
          className="swatch"
          aria-checked={props.value === c.value}
          aria-label={props.names[c.value]}
          title={props.names[c.value]}
          onClick={() => props.onChange(c.value)}
        >
          <Pointer color={c.hex} />
        </button>
      ))}
    </div>
  );
}

const sameApp = (a: { id: string }, b: { id: string }) => a.id.toLowerCase() === b.id.toLowerCase();

type PickerState = { phase: "loading" } | { phase: "failed"; missing: boolean } | { phase: "ready"; apps: AppChoice[] };

/** The apps of this computer, to tick. The list comes from the helper and
 * takes a few seconds; it is read when the sheet opens. */
function AppPicker(props: { policy: "except" | "only"; picked: AppRef[]; onToggle: (app: AppChoice) => void; onClose: () => void }) {
  const { s } = useStore();
  const t = s.computer;
  const [list, setList] = useState<PickerState>({ phase: "loading" });
  const [query, setQuery] = useState("");
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let alive = true;
    setList({ phase: "loading" });
    backend
      .computerApps()
      .then((found) => {
        if (alive) setList({ phase: "ready", apps: found.apps });
      })
      .catch((e: unknown) => {
        if (alive) setList({ phase: "failed", missing: String(e instanceof Error ? e.message : e).includes("not_installed") });
      });
    return () => {
      alive = false;
    };
  }, [attempt]);

  const shown = useMemo(() => {
    if (list.phase !== "ready") return [];
    const q = query.trim().toLowerCase();
    return q ? list.apps.filter((a) => a.name.toLowerCase().includes(q) || a.id.toLowerCase().includes(q)) : list.apps;
  }, [list, query]);

  return (
    <Sheet
      title={props.policy === "except" ? t.pickerTitleExcept : t.pickerTitleOnly}
      onCancel={props.onClose}
      actions={
        <button type="button" className="btn primary" onClick={props.onClose}>
          {t.pickerDone}
        </button>
      }
    >
      <input
        className="input picker-search"
        type="search"
        data-autofocus
        value={query}
        placeholder={t.pickerSearch}
        aria-label={t.pickerSearch}
        spellCheck={false}
        autoCapitalize="off"
        autoCorrect="off"
        onChange={(e) => setQuery(e.target.value)}
      />
      <div className="picker-list" role="group" aria-label={props.policy === "except" ? t.pickerTitleExcept : t.pickerTitleOnly} aria-busy={list.phase === "loading"}>
        {list.phase === "loading" ? (
          <div className="empty" role="status">
            {t.pickerLoading}
          </div>
        ) : list.phase === "failed" ? (
          <div className="empty" role="alert">
            {list.missing ? t.pickerMissing : t.pickerFailed}
            {list.missing ? null : (
              <div style={{ marginTop: 8 }}>
                <button type="button" className="btn small" onClick={() => setAttempt((n) => n + 1)}>
                  {s.common.retry}
                </button>
              </div>
            )}
          </div>
        ) : shown.length === 0 ? (
          <div className="empty">{t.pickerNone}</div>
        ) : (
          shown.map((app) => {
            const on = props.picked.some((p) => !p.loose && sameApp(p, app));
            return (
              <label className="picker-row" key={app.id}>
                <input type="checkbox" checked={on} onChange={() => props.onToggle(app)} />
                <span className="item-text" title={app.path ?? app.id}>
                  {app.name}
                </span>
                {app.sensitive ? <span className="tag">{t.pickerSensitive}</span> : null}
                {app.running ? <span className="tag">{t.pickerRunning}</span> : null}
              </label>
            );
          })
        )}
      </div>
    </Sheet>
  );
}

/** Which apps the synth may use: every app, every app except some, only
 * some. The list of the chosen kind is edited under the choice. */
function WhichApps() {
  const { state, s, name, edit } = useStore();
  const t = s.computer;
  const scope = state.core.settings.computer.apps;
  const [picking, setPicking] = useState(false);
  const [prefilled, setPrefilled] = useState(false);
  const policy = scope.policy;
  const list = policy === "only" ? scope.only : scope.except;

  const choose = (next: AppsPolicy) => {
    edit((d) => void (d.computer.apps.policy = next));
    setPrefilled(false);
    // A list of exceptions that has nothing on it yet starts with the
    // password managers and security settings this computer has.
    if (next === "except" && scope.except.length === 0) {
      void backend
        .computerApps()
        .then((found) => {
          if (found.suggested.length === 0) return;
          edit((d) => {
            const apps = d.computer.apps;
            if (apps.policy === "except" && apps.except.length === 0) apps.except = found.suggested;
          });
          setPrefilled(true);
        })
        .catch(() => undefined);
    }
  };

  const toggle = (app: AppChoice) =>
    edit((d) => {
      const apps = d.computer.apps;
      const key = apps.policy === "only" ? "only" : "except";
      const has = apps[key].some((p) => !p.loose && sameApp(p, app));
      apps[key] = has ? apps[key].filter((p) => p.loose || !sameApp(p, app)) : [...apps[key], { id: app.id, name: app.name }];
    });

  const remove = (app: AppRef) =>
    edit((d) => {
      const apps = d.computer.apps;
      const key = apps.policy === "only" ? "only" : "except";
      apps[key] = apps[key].filter((p) => !(sameApp(p, app) && Boolean(p.loose) === Boolean(app.loose)));
    });

  const editor =
    policy === "all" ? undefined : (
      <div className="group app-list" style={{ marginTop: 8 }}>
        {list.length === 0 ? (
          <div className="empty">{policy === "only" ? t.appsEmptyOnly(name) : t.appsEmptyExcept}</div>
        ) : (
          list.map((app) => (
            <div className="item" key={`${app.loose ? "w" : "a"}:${app.id}`}>
              <span className="item-text selectable" title={app.loose ? t.appsLooseTitle : app.id}>
                {app.name || app.id}
              </span>
              {app.loose ? (
                <span className="tag" title={t.appsLooseTitle}>
                  {t.appsLoose}
                </span>
              ) : null}
              <button type="button" className="btn quiet icon-only" aria-label={`${s.common.remove}: ${app.name || app.id}`} title={s.common.remove} onClick={() => remove(app)}>
                <IconX />
              </button>
            </div>
          ))
        )}
        <div className="add-row">
          <button type="button" className="btn" onClick={() => setPicking(true)}>
            <IconPlus />
            {t.appsAdd}
          </button>
        </div>
      </div>
    );

  return (
    <Section title={t.appsTitle} note={t.appsNote(name)}>
      <ChoiceList<AppsPolicy>
        label={t.appsTitle}
        value={policy}
        onChange={choose}
        options={[
          { value: "all", title: t.appsAll, desc: t.appsAllDesc(name) },
          { value: "except", title: t.appsExcept, desc: t.appsExceptDesc(name) },
          { value: "only", title: t.appsOnly, desc: t.appsOnlyDesc(name) },
        ]}
      />
      {editor}
      {policy === "except" && prefilled ? (
        <div style={{ marginTop: 8 }}>
          <Notice kind="info">{t.appsPrefilled(name)}</Notice>
        </div>
      ) : null}
      {policy !== "all" ? (
        <div style={{ marginTop: 8 }}>
          <Notice kind="info">{policy === "only" ? t.appsLimitsOnly(name) : t.appsLimitsExcept(name)}</Notice>
        </div>
      ) : null}
      {picking && policy !== "all" ? <AppPicker policy={policy} picked={list} onToggle={toggle} onClose={() => setPicking(false)} /> : null}
    </Section>
  );
}

export function Computer() {
  const { state, s, name, edit } = useStore();
  const c = state.core.settings.computer;
  const look = c.look;
  const can = state.look_support;
  const t = s.computer;
  const minutes = [...new Set([...AWAY_MINUTES, c.away_idle_minutes])].sort((a, b) => a - b);
  // Open from the start when something in it is not the default.
  const [advanced, setAdvanced] = useState(look.screenshot_max !== 0 || look.preview_window);

  const awaySelect = (
    <>
      {t.awayAfter}
      <select
        className="select"
        aria-label={t.awayAfter}
        value={c.away_idle_minutes}
        onChange={(e) => edit((d) => void (d.computer.away_idle_minutes = Number(e.target.value)))}
      >
        {minutes.map((m) => (
          <option key={m} value={m}>
            {s.units.minutes(m)}
          </option>
        ))}
      </select>
    </>
  );

  return (
    <>
      <h1 className="page-title">{s.nav.computer}</h1>
      <p className="page-lead">{t.lead(name)}</p>

      <Section>
        <div className="group">
          <SwitchRow strong title={t.enable(name)} checked={c.enabled} onChange={(v) => edit((d) => void (d.computer.enabled = v))} />
        </div>
        <div style={{ marginTop: 8 }}>
          <Notice kind={c.enabled ? "warn" : "info"}>{t.reach(name)}</Notice>
        </div>
        {state.app.os === "macos" ? (
          <div style={{ marginTop: 8 }}>
            <SwitchRow
              title={t.keepAwakeTitle}
              desc={t.keepAwakeDesc}
              checked={c.keep_awake}
              disabled={!c.enabled}
              onChange={(v) => edit((d) => void (d.computer.keep_awake = v))}
            />
          </div>
        ) : null}
      </Section>

      <fieldset disabled={!c.enabled}>
        <Section title={t.modeTitle}>
          <ChoiceList<ComputerMode>
            label={t.modeTitle}
            value={c.mode}
            onChange={(v) => edit((d) => void (d.computer.mode = v))}
            options={[
              { value: "ask", title: t.ask, desc: t.askDesc(name) },
              { value: "auto", title: t.auto, desc: t.autoDesc(name) },
              { value: "away", title: t.away, desc: t.awayDesc(name), extra: awaySelect },
            ]}
          />
        </Section>

        <Section title={t.fgTitle} note={c.foreground === "idle" && c.mode !== "away" ? awaySelect : undefined}>
          <div className="group">
            <Row title={t.fgLead}>
              <select
                className="select"
                aria-label={t.fgTitle}
                value={c.foreground}
                onChange={(e) => edit((d) => void (d.computer.foreground = e.target.value as Foreground))}
              >
                <option value="never">{t.fgNever}</option>
                <option value="idle">{t.fgIdle}</option>
                <option value="ask">{t.fgAsk}</option>
                <option value="always">{t.fgAlways}</option>
              </select>
            </Row>
          </div>
        </Section>

        <WhichApps />

        <Section title={t.lookTitle} note={t.lookNote(name)}>
          <div className="group">
            <SwitchRow
              title={t.cursor(name)}
              // Where the desktop draws the pointer itself, the task name may not be shown.
              desc={can.motion ? t.cursorDesc(name) : t.cursorDescPlain(name)}
              checked={look.cursor}
              onChange={(v) => edit((d) => void (d.computer.look.cursor = v))}
            />
            {look.cursor && can.color ? (
              <Row title={t.color} desc={look.color === "auto" ? t.colorAutoDesc : t.colorNames[look.color]}>
                <ColorChoice
                  label={t.color}
                  value={look.color}
                  names={t.colorNames}
                  onChange={(v) => edit((d) => void (d.computer.look.color = v))}
                />
              </Row>
            ) : null}
            {look.cursor && can.motion ? (
              <>
                <Row title={t.speed} desc={look.speed === "relaxed" ? t.speedDesc.relaxed(name) : t.speedDesc[look.speed]}>
                  <Segmented<CursorSpeed>
                    label={t.speed}
                    value={look.speed}
                    onChange={(v) => edit((d) => void (d.computer.look.speed = v))}
                    options={[
                      { value: "fast", title: t.speedFast },
                      { value: "normal", title: t.speedNormal },
                      { value: "relaxed", title: t.speedRelaxed },
                    ]}
                  />
                </Row>
                <Row title={t.idleHide}>
                  <select
                    className="select"
                    aria-label={t.idleHide}
                    value={look.idle_hide_seconds}
                    onChange={(e) => edit((d) => void (d.computer.look.idle_hide_seconds = Number(e.target.value)))}
                  >
                    {IDLE_HIDE.map((seconds) => (
                      <option key={seconds} value={seconds}>
                        {seconds === 0 ? t.idleHideNever : t.idleHideAfter(seconds)}
                      </option>
                    ))}
                  </select>
                </Row>
                <Row title={t.motion} desc={t.motionDesc}>
                  <select
                    className="select"
                    aria-label={t.motion}
                    value={look.reduced_motion}
                    onChange={(e) => edit((d) => void (d.computer.look.reduced_motion = e.target.value as ReducedMotion))}
                  >
                    <option value="system">{t.motionSystem}</option>
                    <option value="always">{t.motionStill}</option>
                    <option value="never">{t.motionAnimated}</option>
                  </select>
                </Row>
              </>
            ) : null}
          </div>

          <details className="more" open={advanced} onToggle={(e) => setAdvanced(e.currentTarget.open)}>
            <summary>{t.advanced}</summary>
            <div className="group more-group">
              <Row title={t.shot(name)} desc={t.shotDesc(name)}>
                <select
                  className="select"
                  aria-label={t.shot(name)}
                  value={look.screenshot_max}
                  onChange={(e) => edit((d) => void (d.computer.look.screenshot_max = Number(e.target.value)))}
                >
                  {SCREENSHOTS.map((px) => (
                    <option key={px} value={px}>
                      {px === 0 ? t.shotAuto : px === 1024 ? t.shotSmall : px === 1568 ? t.shotMedium : t.shotLarge}
                    </option>
                  ))}
                </select>
              </Row>
              {can.preview_window ? (
                <SwitchRow
                  title={t.preview}
                  tag={t.experimental}
                  desc={t.previewDesc(name)}
                  checked={look.preview_window}
                  onChange={(v) => edit((d) => void (d.computer.look.preview_window = v))}
                />
              ) : null}
            </div>
          </details>
        </Section>

        <Section title={t.expTitle}>
          <div className="group">
            <Row title={t.expLead(name)} desc={t.expDesc(name)}>
              <Segmented<Experiment>
                label={t.expTitle}
                value={c.experiment}
                onChange={(v) => edit((d) => void (d.computer.experiment = v))}
                options={[
                  { value: "default", title: t.expDefault },
                  { value: "on", title: t.expOn },
                  { value: "off", title: t.expOff },
                ]}
              />
            </Row>
          </div>
        </Section>
      </fieldset>

      <Remembered capability="computer" />

      <Section title={t.needsTitle}>
        <HelperStatus live={false} />
      </Section>
    </>
  );
}
