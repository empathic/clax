import { NAME_FAILED, NAME_LOAD_FAILED, report, scopedNotice } from "../failure";
import { type Viewer, getViewer, setViewerName } from "../threads";

export type SetNotice = (u: string | null | ((prev: string | null) => string | null)) => void;

/** The "Your name" field's behaviour. A failed load or save shows in the
 * notice; a successful save clears either. A save waits for the initial
 * lookup, which sets the viewer cookie, so the two never create two viewers. */
export class NameSaver {
  private saved = "";
  private loaded: Promise<unknown> = Promise.resolve();
  private edited = false;

  constructor(private readonly setNotice: SetNotice, private readonly onViewer?: (v: Viewer) => void) {}

  /** Starts the lookup; `show` gets the stored name unless the viewer typed first. */
  load(show: (name: string) => void): void {
    this.loaded = report(getViewer(), NAME_LOAD_FAILED, scopedNotice(this.setNotice, NAME_LOAD_FAILED)).then(v => {
      if (!v) return;
      this.onViewer?.(v);
      this.saved = v.display_name ?? "";
      if (!this.edited) show(v.display_name ?? "");
    });
  }

  /** The viewer typed in the field. */
  edit(): void {
    this.edited = true;
  }

  /** Saves `name` (trimmed) once the lookup answered, unless it is the stored name. */
  save(name: string): void {
    const next = name.trim();
    void this.loaded.then(() => {
      if (next === this.saved) return;
      void report(setViewerName(next), NAME_FAILED, scopedNotice(this.setNotice, NAME_FAILED, NAME_LOAD_FAILED)).then(v => {
        if (v) { this.onViewer?.(v); this.saved = v.display_name ?? ""; }
      });
    });
  }
}
