import { useEffect, useState } from "preact/hooks";
import { NAME_FAILED, report } from "./failure";
import { getViewer, setViewerName } from "./threads";

/** The header's "Your name" field; saves on Enter or blur and reports a failed save through `setNotice`. */
export function ViewerName({ setNotice }: { setNotice(text: string | null): void }) {
  const [name, setName] = useState("");
  const [saved, setSaved] = useState("");
  useEffect(() => {
    void report(getViewer(), NAME_FAILED, setNotice).then(v => { if (v) { setName(v.display_name ?? ""); setSaved(v.display_name ?? ""); } });
  }, []);
  const save = () => {
    if (name.trim() === saved) return;
    void report(setViewerName(name.trim()), NAME_FAILED, setNotice).then(v => { if (v) setSaved(v.display_name ?? ""); });
  };
  return (
    <input class="viewer-name hide-sm" aria-label="Your name" placeholder="Your name" value={name} maxLength={60}
      onInput={e => setName((e.target as HTMLInputElement).value)} onBlur={save} onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); save(); } }} />
  );
}
