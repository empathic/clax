// The side panel's link to the worker (spec 2026-10-05 §9.4, §9.5): one
// port per window, named `panel:<windowId>`; the state of the window's
// active tab; whether the worker's stream is up; whether the panel is
// visible (the worker reports the owner here only while it is); and a ping
// every 20 s that keeps the worker up while the panel is open. Only
// messages `isToPanel` takes are acted on. When the worker goes away (Chrome
// stopped it), the link connects again and watches the tab again. An
// action's failure stays shown through the worker's later pushes until the
// person acts again or watches another tab.
import { type PanelState, type PanelToWorker, isToPanel } from "../messages";

type Port = Pick<chrome.runtime.Port, "postMessage" | "onMessage" | "onDisconnect" | "disconnect">;
export type LinkEnv = {
  runtime: { connect(info: { name: string }): Port };
  tabs: Pick<typeof chrome.tabs, "query" | "onActivated" | "onUpdated">;
  /** The panel page's query: `?tab=<id>` pins it to one tab. */
  search: string;
  doc: Pick<Document, "visibilityState" | "addEventListener" | "removeEventListener">;
};
const chromeEnv = (): LinkEnv => ({ runtime: chrome.runtime, tabs: chrome.tabs, search: location.search, doc: document });

export const PING_MS = 20_000;
/** How long the link waits before connecting again after the worker went away. */
export const RECONNECT_MS = 500;

export class PanelLink {
  state = $state<PanelState | null>(null);
  /** Whether the worker's event stream is up. */
  up = $state(true);
  private port: Port;
  private tabId: number | null = null;
  /** The last action's failure, shown until the next action. */
  private failure: { code: string; message: string } | null = null;
  private closed = false;
  /** The port is new: its first watch says whether the panel is visible. */
  private fresh = true;
  private readonly pinned: number | null;
  private readonly beat: ReturnType<typeof setInterval>;
  private readonly onVisibility = () => this.post({ t: "visible", on: this.env.doc.visibilityState === "visible" });
  private readonly onActivated = (i: { windowId: number }) => { if (i.windowId === this.windowId) void this.follow(); };
  private readonly onUpdated = (_id: number, c: { url?: string }, tab: { active: boolean; windowId: number }) => {
    if (tab.active && tab.windowId === this.windowId && c.url) void this.follow();
  };

  constructor(private readonly windowId: number, private readonly env: LinkEnv = chromeEnv()) {
    // `?tab=<id>` pins the panel to one tab (the browser test opens the
    // panel's page in a tab of its own); otherwise it follows the window's
    // active tab.
    const pinned = Number(new URLSearchParams(env.search).get("tab"));
    this.pinned = Number.isSafeInteger(pinned) && pinned > 0 ? pinned : null;
    if (this.pinned === null) {
      env.tabs.onActivated.addListener(this.onActivated);
      env.tabs.onUpdated.addListener(this.onUpdated as never);
    }
    env.doc.addEventListener("visibilitychange", this.onVisibility);
    this.beat = setInterval(() => this.post({ t: "ping" }), PING_MS);
    this.port = this.connect();
    void this.follow();
  }

  private connect(): Port {
    const port = this.env.runtime.connect({ name: `panel:${this.windowId}` });
    this.fresh = true;
    port.onMessage.addListener((m: unknown) => {
      if (!isToPanel(m)) return;
      if (m.t === "tab") this.state = this.failure && !m.state.error ? { ...m.state, error: this.failure } : m.state;
      else if (m.t === "failed" && this.state) {
        this.failure = { code: m.code, message: m.message };
        this.state = { ...this.state, error: this.failure };
      }
      else if (m.t === "stream-status") this.up = m.up;
    });
    port.onDisconnect.addListener(() => {
      if (this.closed) return;
      setTimeout(() => {
        if (this.closed) return;
        this.port = this.connect();
        if (this.tabId !== null) this.watch(this.tabId);
        else void this.follow();
      }, RECONNECT_MS);
    });
    return port;
  }

  private watch(tabId: number): void {
    if (tabId !== this.tabId) this.failure = null;
    this.tabId = tabId;
    this.post({ t: "watch-tab", tabId });
    if (this.fresh) { this.fresh = false; this.onVisibility(); }
  }

  private async follow(): Promise<void> {
    if (this.pinned !== null) { this.watch(this.pinned); return; }
    const [tab] = await this.env.tabs.query({ active: true, windowId: this.windowId });
    if (tab?.id !== undefined && !this.closed) this.watch(tab.id);
  }

  post(m: PanelToWorker): void {
    if (this.closed) return;
    if (m.t !== "ping" && m.t !== "visible" && m.t !== "watch-tab") this.failure = null;
    try { this.port.postMessage(m); } catch { /* the port closed; the link connects again */ }
  }

  /** Stops the link (tests). */
  close(): void {
    this.closed = true;
    clearInterval(this.beat);
    this.env.tabs.onActivated.removeListener?.(this.onActivated);
    this.env.tabs.onUpdated.removeListener?.(this.onUpdated as never);
    this.env.doc.removeEventListener("visibilitychange", this.onVisibility);
    this.port.disconnect();
  }
}
