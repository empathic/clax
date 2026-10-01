import { useEffect, useMemo, useState } from "preact/hooks";
import type { Viewer } from "./threads";
import { NameSaver, type SetNotice } from "./view/viewer-name-model";

/** The "Your name" field; saves on Enter or blur through a `NameSaver`. */
export function ViewerName({ setNotice, onViewer }: { setNotice: SetNotice; onViewer?(v: Viewer): void }) {
  const [name, setName] = useState("");
  const saver = useMemo(() => new NameSaver(setNotice, onViewer), []);
  useEffect(() => saver.load(setName), []);
  const save = () => saver.save(name);
  return (
    <input class="viewer-name" aria-label="Your name" placeholder="Your name" value={name} maxLength={60}
      onInput={e => { saver.edit(); setName((e.target as HTMLInputElement).value); }} onBlur={save} onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); save(); } }} />
  );
}
