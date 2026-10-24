// The answers the owner asked the app to remember for one ability, each
// with a way to take it back. Sites have their own list on the Sites and
// cookies page; this one serves Computer control and Files.

import { backend } from "../backend";
import type { Grant } from "../backend/types";
import { grantTime, shortPath } from "../lib/format";
import { grantSynth, useStore } from "../lib/store";
import { Section } from "../ui/controls";

export function Remembered(props: { capability: "computer" | "files" | "shell"; title?: string }) {
  const { state, s, lang, act } = useStore();
  const grants = state.grants.filter((g) => g.capability === props.capability);
  if (grants.length === 0) return null;

  const label = (g: Grant): string => {
    if (props.capability === "files") return shortPath(g.scope);
    if (props.capability === "shell") return g.scope;
    if (g.scope === "task") return s.remembered.task;
    if (g.scope.startsWith("foreground:")) {
      const tool = g.scope.slice("foreground:".length);
      return s.remembered.screen((s.journal.tool[tool] ?? tool).toLowerCase());
    }
    return g.scope;
  };

  return (
    <Section title={props.title ?? s.remembered.title}>
      <div className="group">
        {grants.map((g) => (
          <div className="item" key={`${g.synth_id}/${g.scope}`}>
            <span
              className={`item-text${props.capability === "shell" ? " selectable mono" : ""}`}
              title={props.capability === "files" ? g.scope : undefined}
            >
              {label(g)}
            </span>
            {grantSynth(state, g) ? <span className="tag">{grantSynth(state, g)}</span> : null}
            <span className="item-sub">{grantTime(g, s, lang)}</span>
            <button type="button" className="btn small" onClick={() => void act(() => backend.revokeGrant(g.capability, g.scope, g.synth_id))}>
              {s.cookies.revoke}
            </button>
          </div>
        ))}
      </div>
    </Section>
  );
}
