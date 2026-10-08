import { afterEach, describe, expect, it, vi } from "vitest";
import { setBadge } from "./badge";
import { item, view } from "./fixtures";
import { NOTIFY_ICON, Notifier } from "./notify";

/** `Notification` with permission `p`: every one made, and whether it closed. */
function fakeNotification(p: NotificationPermission) {
  const made: FakeN[] = [];
  class FakeN {
    static permission = p;
    onclick: (() => void) | null = null;
    onclose: (() => void) | null = null;
    closed = false;
    tag: string;
    constructor(public title: string, public opts: NotificationOptions) { this.tag = opts.tag ?? ""; made.push(this); }
    close() { this.closed = true; this.onclose?.(); }
  }
  return { N: FakeN as unknown as typeof Notification, made };
}

afterEach(() => { vi.restoreAllMocks(); });

describe("Notifier", () => {
  it("shows an unread item with its title, body, icon and tag; a click focuses, opens and marks it read", () => {
    const { N, made } = fakeNotification("granted");
    const open = vi.fn();
    const mark = vi.fn();
    const focus = vi.spyOn(globalThis, "focus").mockImplementation(() => {});
    const n = new Notifier(N, open, mark);
    n.show(item("reply"));
    expect(made).toHaveLength(1);
    expect(made[0].title).toBe("claude replied on Quarterly Review");
    expect(made[0].opts).toEqual({ body: "Done: two columns now. The table sorts too.", tag: "clax-inbox-7q3k9mzx2b4t", icon: NOTIFY_ICON });
    made[0].onclick!();
    expect(focus).toHaveBeenCalled();
    expect(open).toHaveBeenCalledWith("/a/7q3k9mzx2b4t?thread=01J9T0000000000000000000AA");
    expect(mark).toHaveBeenCalledWith("01JA00000000000000000000AA");
    expect(made[0].closed).toBe(true);
  });

  it("tags a question by the question, closes a notification once its item is read, and closes them all", () => {
    const { N, made } = fakeNotification("granted");
    const n = new Notifier(N, vi.fn(), vi.fn());
    n.show(item("question", { id: "I2" }));
    expect(made[0].tag).toBe(`clax-inbox-${view().id}`);
    expect(made[0].title).toBe("claude asks: Layout, Sidebars, Notes");
    n.close("I1");
    expect(made[0].closed).toBe(false);
    n.close("I2");
    expect(made[0].closed).toBe(true);
    n.show(item("reply", { id: "I3" }));
    n.show(item("published", { id: "I4" }));
    n.closeAll();
    expect(made.slice(1).map(m => m.closed)).toEqual([true, true]);
  });

  it("strips control and bidirectional characters from what it shows", () => {
    const { N, made } = fakeNotification("granted");
    new Notifier(N, vi.fn(), vi.fn()).show(item("reply", { reply: { comment_id: "c", body: "‮evil\u0007 done", addressed: false } }));
    expect(made[0].opts.body).toBe("evil done");
  });

  it("shows nothing without permission, without notifications, or for a read item", () => {
    for (const p of ["default", "denied"] as const) {
      const { N, made } = fakeNotification(p);
      new Notifier(N, vi.fn(), vi.fn()).show(item("reply"));
      expect(made, p).toEqual([]);
    }
    expect(() => new Notifier(undefined, vi.fn(), vi.fn()).show(item("reply"))).not.toThrow();
    const { N, made } = fakeNotification("granted");
    new Notifier(N, vi.fn(), vi.fn()).show(item("reply", { read: true }));
    expect(made).toEqual([]);
  });
});

describe("setBadge", () => {
  it("prefixes the title with the count and swaps the icon, and puts both back at 0", () => {
    document.head.innerHTML = `<title>Quarterly Review</title><link rel="icon" type="image/svg+xml" href="/_clax/mark.svg">`;
    const icon = () => document.querySelector("link[rel=icon]")!.getAttribute("href");
    setBadge(document, 2);
    expect(document.title).toBe("(2) Quarterly Review");
    expect(icon()).toBe("/_clax/mark-dot.svg");
    setBadge(document, 13);
    expect(document.title).toBe("(13) Quarterly Review");
    setBadge(document, 0);
    expect(document.title).toBe("Quarterly Review");
    expect(icon()).toBe("/_clax/mark.svg");
    setBadge(document, 0);
    expect(icon()).toBe("/_clax/mark.svg");
  });
});
