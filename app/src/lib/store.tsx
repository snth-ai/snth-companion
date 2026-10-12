// The UI's single source of truth: the state object the shell sends, the
// journal, and the few helpers every screen uses to change them.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { backend } from "../backend";
import type { Entry, Settings, UiState } from "../backend/types";
import { I18nContext, resolveLang, stringsFor, type Lang, type Strings } from "../i18n";

/** How long after its journal line an action still counts as "right now". */
const ACTIVE_SECONDS = 8;
const JOURNAL_KEEP = 800;

export interface Store {
  state: UiState;
  journal: Entry[];
  lang: Lang;
  s: Strings;
  /** The synth's name for sentences: its name, else its id. */
  name: string;
  /** A code that arrived through a `snth://pair` link, not handled yet. */
  linkCode: string | null;
  takeLinkCode: () => void;
  /** Put a state object returned by a command in place. */
  set: (next: UiState) => void;
  /** Run a command that returns the new state. False if it failed. */
  act: (work: () => Promise<UiState | null | void>) => Promise<boolean>;
  /** Change settings: shown at once, sent to the shell in order. */
  edit: (change: (draft: Settings) => void) => void;
  /** The last command failed (shown as one plain line by the page). */
  failed: boolean;
}

const StoreContext = createContext<Store | null>(null);

export function useStore(): Store {
  const store = useContext(StoreContext);
  if (!store) throw new Error("no store");
  return store;
}

export function synthName(state: UiState, s: Strings): string {
  return state.core.synth_name || state.core.synth_id || state.pending?.synth_id || s.common.yourSynth;
}

/** Re-render on a timer (countdowns, "right now"). */
export function useNow(everyMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = window.setInterval(() => setNow(Date.now()), everyMs);
    return () => window.clearInterval(id);
  }, [everyMs]);
  return now;
}

/** The journal line that is happening right now, if any. */
export function useActiveEntry(): Entry | null {
  const { journal, state } = useStore();
  const now = useNow(1000);
  const last = journal[journal.length - 1];
  if (!last || state.core.paused || !state.core.paired) return null;
  // Installing the helper is the app's own work, not the synth at work.
  if (last.action.startsWith("helper.") || last.capability === "app") return null;
  return now / 1000 - last.time <= ACTIVE_SECONDS ? last : null;
}

/** `main` is false in the approval window: it has no journal, and it must
 * not collect a pairing link meant for the main window. */
export function StoreProvider(props: { children: ReactNode; fallback?: ReactNode; main?: boolean; forceLang?: Lang }) {
  const main = props.main ?? true;
  const [state, setState] = useState<UiState | null>(null);
  const [journal, setJournal] = useState<Entry[]>([]);
  const [linkCode, setLinkCode] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const queue = useRef<Promise<unknown>>(Promise.resolve());
  const editSeq = useRef(0);

  const lang = props.forceLang ?? resolveLang(state?.prefs.language);
  const s = stringsFor(lang);
  const latest = useRef({ state, lang });
  latest.current = { state, lang };

  useEffect(() => {
    document.documentElement.lang = lang;
  }, [lang]);

  // Tell the shell which language the UI resolved to (the tray follows
  // it) and pick up a pairing link that arrived before the window did.
  const collectLink = useCallback((language: Lang) => {
    void backend
      .uiReady(language)
      .then((r) => {
        if (r.pair_code) setLinkCode(r.pair_code);
      })
      .catch(() => undefined);
  }, []);

  useEffect(() => {
    let alive = true;
    const refresh = () =>
      backend
        .getState()
        .then((next) => {
          if (alive) setState(next);
        })
        .catch(() => undefined);
    void refresh();
    if (main) {
      void backend
        .getJournal()
        .then((entries) => {
          if (alive) setJournal(entries.slice(-JOURNAL_KEEP));
        })
        .catch(() => undefined);
    }
    const stop = backend.subscribe((event) => {
      if (event.type === "state") void refresh();
      else if (event.type === "update") setState((current) => (current ? { ...current, update: event.status } : current));
      else if (!main) return;
      else if (event.type === "journal") setJournal((j) => [...j, event.entry].slice(-JOURNAL_KEEP));
      else if (event.type === "pair_link") collectLink(latest.current.lang);
    });
    return () => {
      alive = false;
      stop();
    };
  }, [collectLink, main]);

  const ready = state !== null;
  useEffect(() => {
    if (ready && main) collectLink(lang);
  }, [ready, main, lang, collectLink]);

  const act = useCallback(async (work: () => Promise<UiState | null | void>) => {
    try {
      const next = await work();
      if (next) setState(next);
      setFailed(false);
      return true;
    } catch {
      setFailed(true);
      return false;
    }
  }, []);

  const edit = useCallback((change: (draft: Settings) => void) => {
    const current = latest.current.state;
    if (!current) return;
    // A JSON copy: the state is plain data, and older system webviews
    // have no structuredClone.
    const next = JSON.parse(JSON.stringify(current)) as UiState;
    change(next.core.settings);
    latest.current = { ...latest.current, state: next };
    setState(next);
    const settings = next.core.settings;
    const seq = ++editSeq.current;
    // One at a time, in order. Only the answer to the newest change is
    // shown, so a quick second change is not undone by the first answer.
    queue.current = queue.current
      .then(() => backend.updateSettings(settings))
      .then((fresh) => {
        if (seq === editSeq.current) setState(fresh);
        setFailed(false);
      })
      .catch(() => {
        setFailed(true);
        return backend
          .getState()
          .then(setState)
          .catch(() => undefined);
      });
  }, []);

  const store = useMemo<Store | null>(() => {
    if (!state) return null;
    return {
      state,
      journal,
      lang,
      s,
      name: synthName(state, s),
      linkCode,
      takeLinkCode: () => setLinkCode(null),
      set: setState,
      act,
      edit,
      failed,
    };
  }, [state, journal, lang, s, linkCode, act, edit, failed]);

  if (!store) return <>{props.fallback ?? null}</>;
  return (
    <I18nContext.Provider value={{ lang, s }}>
      <StoreContext.Provider value={store}>{props.children}</StoreContext.Provider>
    </I18nContext.Provider>
  );
}
