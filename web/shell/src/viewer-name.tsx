import { useEffect, useRef, useState } from "preact/hooks";
import { NAME_FAILED, NAME_LOAD_FAILED, report, scopedNotice } from "./failure";
import { getViewer, setViewerName } from "./threads";

type SetNotice = (u: string | null | ((prev: string | null) => string | null)) => void;

/** The "Your name" field; saves on Enter or blur. A failed load or save shows
 * in the notice; a successful save clears either. A save waits for the initial
 * lookup, which sets the viewer cookie, so the two never create two viewers. */
export function ViewerName({ setNotice }: { setNotice: SetNotice }) {
  const [name, setName] = useState("");
  const saved = useRef("");
  const loaded = useRef<Promise<unknown>>(Promise.resolve());
  const edited = useRef(false);
  useEffect(() => {
    const p = report(getViewer(), NAME_LOAD_FAILED, scopedNotice(setNotice, NAME_LOAD_FAILED)).then(v => {
      if (!v) return;
      saved.current = v.display_name ?? "";
      if (!edited.current) setName(v.display_name ?? "");
    });
    loaded.current = p;
  }, []);
  const save = () => {
    const next = name.trim();
    void loaded.current.then(() => {
      if (next === saved.current) return;
      void report(setViewerName(next), NAME_FAILED, scopedNotice(setNotice, NAME_FAILED, NAME_LOAD_FAILED)).then(v => { if (v) saved.current = v.display_name ?? ""; });
    });
  };
  return (
    <input class="viewer-name" aria-label="Your name" placeholder="Your name" value={name} maxLength={60}
      onInput={e => { edited.current = true; setName((e.target as HTMLInputElement).value); }} onBlur={save} onKeyDown={e => { if (e.key === "Enter") { e.preventDefault(); save(); } }} />
  );
}
