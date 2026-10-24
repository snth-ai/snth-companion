import { useState } from "react";
import { backend } from "../backend";
import { shortPath } from "../lib/format";
import { useStore } from "../lib/store";
import { Notice, Section, Segmented, SwitchRow } from "../ui/controls";
import { IconFolder, IconPlus, IconX } from "../ui/icons";
import { Remembered } from "./Remembered";

/** The list of shared folders with its read / change switch. Also used in
 * onboarding. */
export function FolderList() {
  const { state, s, name, act } = useStore();
  const t = s.files;
  const roots = state.core.settings.files.roots;
  const [picking, setPicking] = useState(false);

  const addFolder = async () => {
    setPicking(true);
    await act(() => backend.pickFolder(true));
    setPicking(false);
  };

  return (
    <div className="group">
      {roots.length === 0 ? <div className="empty">{t.empty(name)}</div> : null}
      {roots.map((r) => (
        <div className="item" key={r.path}>
          <IconFolder />
          <span className="item-text" title={r.path}>
            {shortPath(r.path)}
          </span>
          <Segmented
            label={r.path}
            value={r.write ? "write" : "read"}
            options={[
              { value: "read", title: t.readOnly },
              { value: "write", title: t.readWrite },
            ]}
            onChange={(v) => void act(() => backend.setRootWrite(r.path, v === "write"))}
          />
          <button
            type="button"
            className="btn quiet icon-only"
            aria-label={`${s.common.remove}: ${r.path}`}
            title={s.common.remove}
            onClick={() => void act(() => backend.removeRoot(r.path))}
          >
            <IconX />
          </button>
        </div>
      ))}
      <div className="add-row">
        <button type="button" className="btn" disabled={picking} onClick={() => void addFolder()}>
          <IconPlus />
          {t.addFolder}
        </button>
      </div>
    </div>
  );
}

export function Files() {
  const { state, s, name, edit } = useStore();
  const settings = state.core.settings;
  const f = settings.files;
  const t = s.files;

  return (
    <>
      <h1 className="page-title">{s.nav.files}</h1>
      <p className="page-lead">{t.lead(name)}</p>

      <Section>
        <div className="group">
          <SwitchRow strong title={t.enable(name)} checked={f.enabled} onChange={(v) => edit((d) => void (d.files.enabled = v))} />
        </div>
        {settings.computer.enabled ? (
          <div style={{ marginTop: 8 }}>
            <Notice kind="warn">{t.reach(name)}</Notice>
          </div>
        ) : null}
      </Section>

      <fieldset disabled={!f.enabled}>
        <Section title={t.foldersTitle} note={t.noDelete(name)}>
          <FolderList />
        </Section>

        <Section>
          <div className="group">
            <SwitchRow
              title={t.askWrite(name)}
              desc={t.askWriteDesc}
              checked={f.ask_on_write}
              onChange={(v) => edit((d) => void (d.files.ask_on_write = v))}
            />
          </div>
        </Section>
      </fieldset>

      <Remembered capability="files" />
    </>
  );
}
