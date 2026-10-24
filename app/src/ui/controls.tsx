// The small set of controls every screen is built from.

import { useEffect, useRef, useState, type ReactNode } from "react";
import { IconInfo, IconPlus, IconWarning, IconX } from "./icons";

export function Switch(props: { checked: boolean; onChange: (next: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      type="button"
      role="switch"
      className="switch"
      aria-checked={props.checked}
      aria-label={props.label}
      disabled={props.disabled}
      onClick={() => props.onChange(!props.checked)}
    />
  );
}

/** A row with a title, an optional description and a control on the right. */
export function Row(props: { title: ReactNode; desc?: ReactNode; strong?: boolean; children?: ReactNode; icon?: ReactNode }) {
  return (
    <div className="row">
      {props.icon}
      <div className="row-text">
        <div className={`row-title${props.strong ? " strong" : ""}`}>{props.title}</div>
        {props.desc ? <div className="row-desc">{props.desc}</div> : null}
      </div>
      {props.children}
    </div>
  );
}

export function SwitchRow(props: {
  title: string;
  desc?: ReactNode;
  checked: boolean;
  onChange: (next: boolean) => void;
  strong?: boolean;
  disabled?: boolean;
  /** A short mark after the title ("Experimental"). */
  tag?: string;
}) {
  const title = props.tag ? (
    <>
      {props.title} <span className="tag">{props.tag}</span>
    </>
  ) : (
    props.title
  );
  return (
    <Row title={title} desc={props.desc} strong={props.strong}>
      <Switch checked={props.checked} onChange={props.onChange} label={props.title} disabled={props.disabled} />
    </Row>
  );
}

export function Section(props: { title?: string; note?: ReactNode; children: ReactNode }) {
  return (
    <section className="section">
      {props.title ? <h2 className="section-title">{props.title}</h2> : null}
      {props.children}
      {props.note ? <p className="section-note">{props.note}</p> : null}
    </section>
  );
}

export interface Option<T extends string> {
  value: T;
  title: string;
  desc?: ReactNode;
  /** Shown under the option while it is the chosen one. */
  extra?: ReactNode;
}

/** One choice among a few rows, each with room for a plain description. */
export function ChoiceList<T extends string>(props: {
  label: string;
  value: T;
  options: Option<T>[];
  onChange: (next: T) => void;
}) {
  return (
    <div className="group" role="radiogroup" aria-label={props.label}>
      {props.options.map((o) => (
        <div key={o.value}>
          <button
            type="button"
            role="radio"
            className="choice"
            aria-checked={o.value === props.value}
            onClick={() => props.onChange(o.value)}
          >
            <span className="radio" />
            <span className="row-text">
              <span className="row-title">{o.title}</span>
              {o.desc ? <span className="row-desc" style={{ display: "block" }}>{o.desc}</span> : null}
            </span>
          </button>
          {o.extra && o.value === props.value ? <div className="choice-extra">{o.extra}</div> : null}
        </div>
      ))}
    </div>
  );
}

/** One choice among two or three short labels. */
export function Segmented<T extends string>(props: {
  label: string;
  value: T;
  options: { value: T; title: string }[];
  onChange: (next: T) => void;
}) {
  return (
    <div className="seg" role="radiogroup" aria-label={props.label}>
      {props.options.map((o) => (
        <button key={o.value} type="button" role="radio" aria-checked={o.value === props.value} onClick={() => props.onChange(o.value)}>
          {o.title}
        </button>
      ))}
    </div>
  );
}

export function Notice(props: { kind?: "info" | "warn" | "danger"; title?: string; children: ReactNode; actions?: ReactNode }) {
  const kind = props.kind ?? "info";
  return (
    <div className={`notice ${kind}`} role={kind === "info" ? undefined : "status"}>
      {kind === "info" ? <IconInfo /> : <IconWarning />}
      <div className="notice-body">
        {props.title ? <div className="notice-title">{props.title}</div> : null}
        <div>{props.children}</div>
        {props.actions ? <div className="notice-actions">{props.actions}</div> : null}
      </div>
    </div>
  );
}

export function More(props: { label: string; children: ReactNode }) {
  return (
    <details className="more">
      <summary>{props.label}</summary>
      <div className="more-body selectable">{props.children}</div>
    </details>
  );
}

/** A list the person edits: a field to add an entry, then the entries. */
export function ListEditor(props: {
  items: { key: string; label: string; tag?: string }[];
  placeholder: string;
  addLabel: string;
  removeLabel: string;
  emptyText: string;
  /** Return an error message to refuse the entry, or null to accept it. */
  onAdd: (value: string) => string | null;
  onRemove: (key: string) => void;
}) {
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);

  const submit = () => {
    if (!value.trim()) return;
    const problem = props.onAdd(value.trim());
    setError(problem);
    if (!problem) setValue("");
  };

  return (
    <>
      <div className="group">
        <form
          className="add-row"
          onSubmit={(e) => {
            e.preventDefault();
            submit();
          }}
        >
          <input
            className="input"
            value={value}
            placeholder={props.placeholder}
            aria-label={props.addLabel}
            spellCheck={false}
            autoCapitalize="off"
            autoCorrect="off"
            onChange={(e) => {
              setValue(e.target.value);
              setError(null);
            }}
          />
          <button type="submit" className="btn" disabled={!value.trim()}>
            <IconPlus />
            {props.addLabel}
          </button>
        </form>
        {props.items.length === 0 ? (
          <div className="empty">{props.emptyText}</div>
        ) : (
          props.items.map((item) => (
            <div className="item" key={item.key}>
              <span className="item-text selectable" title={item.label}>
                {item.label}
              </span>
              {item.tag ? <span className="tag">{item.tag}</span> : null}
              <button
                type="button"
                className="btn quiet icon-only"
                aria-label={`${props.removeLabel}: ${item.label}`}
                title={props.removeLabel}
                onClick={() => props.onRemove(item.key)}
              >
                <IconX />
              </button>
            </div>
          ))
        )}
      </div>
      {error ? (
        <p className="error-text" role="alert">
          {error}
        </p>
      ) : null}
    </>
  );
}

export function Keys(props: { keys: string[] }) {
  return (
    <span className="kbd">
      {props.keys.map((k, i) => (
        <kbd key={i}>{k}</kbd>
      ))}
    </span>
  );
}

/** A confirmation that needs the person's full attention (unlinking). It
 * sits over the window, takes focus, and Escape or a click outside
 * cancels. */
export function Sheet(props: { title: string; onCancel: () => void; children: ReactNode; actions: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    ref.current?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") props.onCancel();
      if (e.key !== "Tab" || !ref.current) return;
      // Keep Tab inside the sheet.
      const focusable = ref.current.querySelectorAll<HTMLElement>("button:not(:disabled), input:not(:disabled)");
      const first = focusable[0];
      const last = focusable[focusable.length - 1];
      if (!first || !last) return;
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("keydown", onKey);
      previous?.focus();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div
      className="overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) props.onCancel();
      }}
    >
      <div className="sheet" role="alertdialog" aria-modal="true" aria-label={props.title} ref={ref}>
        <h2>{props.title}</h2>
        {props.children}
        <div className="sheet-actions">{props.actions}</div>
      </div>
    </div>
  );
}
