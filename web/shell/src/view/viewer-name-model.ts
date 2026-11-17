import { NAME_FAILED, NAME_LOAD_FAILED, report, scopedNotice } from "../failure";
import { type Viewer, getViewer, setViewerName } from "../threads";

export type SetNotice = (u: string | null | ((prev: string | null) => string | null)) => void;

/** The "Your name" field's behaviour. A failed load or save shows in the
 * notice; a successful save clears either. A save waits for the initial
 * lookup, which sets the viewer cookie, so the two never create two viewers. */
export class NameSaver {
  private saved = "";
  /** The lookup, then each save in turn. */
  private loaded: Promise<unknown> = Promise.resolve();
  /** The name of the latest save not yet answered. */
  private saving: string | null = null;
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

  /** The name changed elsewhere (another view of this viewer, or the CLI):
   * `show` gets it unless the viewer is typing a name of their own. */
  adopt(name: string, show: (name: string) => void): void {
    this.saved = name;
    if (!this.edited) show(name);
  }

  /** The viewer typed in the field. */
  edit(): void {
    this.edited = true;
  }

  /** Saves `name` (trimmed) once the lookup and every earlier save answered,
   * unless it is the stored name then. A save of the name already being
   * saved (Enter, then the blur that follows) sends nothing more: on a
   * hosted daemon a second request sent before the first answers carries no
   * guest cookie yet and is refused 409 `name_taken`. Saves go in order, so
   * the last name typed is the one stored. */
  save(name: string): void {
    const next = name.trim();
    if (next === this.saving) return;
    this.saving = next;
    this.loaded = this.loaded.then(async () => {
      if (next !== this.saved) {
        const v = await report(setViewerName(next), NAME_FAILED, scopedNotice(this.setNotice, NAME_FAILED, NAME_LOAD_FAILED));
        if (v) { this.onViewer?.(v); this.saved = v.display_name ?? ""; this.edited = false; }
      }
      if (this.saving === next) this.saving = null;
    });
  }
}
