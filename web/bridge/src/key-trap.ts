// The keys the viewer types in the page right after a pick. The composer
// opens in the shell and takes focus a moment after the click or release
// that picks; until focus leaves the page, every key and text event the
// viewer makes there is kept from the page (and from comment mode), and the
// text keys are handed to the shell (`clax:keys`), which puts them in that
// pick's composer ahead of anything typed there. The trap ends when the page
// loses focus (the composer took it) or `TRAP_MS` after the pick, and then
// tells the shell no more keys follow (`done`).
//
// Its listeners are on the window, capturing, and added when the bridge
// loads, before any page script runs, so no listener of the page's hears a
// trapped event. Events the page dispatched itself are neither kept from it
// nor handed on.

/** The longest the trap holds the page's keys after a pick, when focus never
 * leaves the page (the shell refused the pick, or never focused its composer). */
export const TRAP_MS = 2_000;

/** Every key and text event type the trap keeps from the page. */
export const TRAPPED_EVENTS = ["keydown", "keypress", "keyup", "beforeinput", "input", "textInput", "compositionstart", "compositionupdate", "compositionend", "paste", "cut", "copy"] as const;

/** The text a key press types in a text field: its character (one code
 * point; Option or AltGr may make it), "\n" for Enter, "Backspace" for
 * Backspace; null for any other key, a shortcut (Cmd, or Ctrl without
 * AltGr), or a press inside an input method's composition. */
export function keyText(e: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "altKey" | "isComposing">): string | null {
  if (e.isComposing || e.metaKey || (e.ctrlKey && !e.altKey)) return null;
  if (e.key === "Enter") return "\n";
  if (e.key === "Backspace") return "Backspace";
  return Array.from(e.key).length === 1 && !/\p{Cc}/u.test(e.key) ? e.key : null;
}

export class KeyTrap {
  private pickId: string | null = null;
  private timer: ReturnType<typeof setTimeout> | undefined;
  private readonly trustedOnly: boolean;
  private readonly ms: number;

  /** `send` hands keys for `pickId` to the shell; `opts.trustedOnly`
   * (default true) ignores events the page dispatched; tests that
   * synthesise input turn it off. */
  constructor(private readonly win: Window, private readonly send: (pickId: string, keys: string[], done: boolean) => void, opts: { trustedOnly?: boolean; ms?: number } = {}) {
    this.trustedOnly = opts.trustedOnly ?? true;
    this.ms = opts.ms ?? TRAP_MS;
    for (const t of TRAPPED_EVENTS) win.addEventListener(t, this.onEvent, true);
    win.addEventListener("blur", () => this.end());
  }

  /** Traps the keys typed from now on for the pick `pickId` (ending any
   * earlier trap). A page without focus gets no keys: that trap ends at once. */
  start(pickId: string): void {
    this.end();
    this.pickId = pickId;
    if (!this.win.document.hasFocus()) { this.end(); return; }
    this.timer = setTimeout(() => this.end(), this.ms);
  }

  /** Whether keys are being trapped. */
  get active(): boolean {
    return this.pickId !== null;
  }

  private end(): void {
    const id = this.pickId;
    if (id === null) return;
    this.pickId = null;
    clearTimeout(this.timer);
    this.send(id, [], true);
  }

  private onEvent = (e: Event) => {
    if (this.pickId === null || (this.trustedOnly && !e.isTrusted)) return;
    e.preventDefault();
    e.stopImmediatePropagation();
    if (e.type !== "keydown") return;
    const t = keyText(e as KeyboardEvent);
    if (t !== null) this.send(this.pickId, [t], false);
  };
}
