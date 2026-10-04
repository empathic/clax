import { describe, it, expect, onTestFinished } from "vitest";
import { artifactViewHooks, waitFor, ID, artifact, page, viewer, mountView, stubMedia, postReady, buttonNamed, fromFrame, gestureIn, viewerPick, startOf, pick, pointerClick } from "./test/artifact-view";

// The view's picks, composer and comment mode (the rest is in artifact.test.ts).
describe("ArtifactView", () => {
  artifactViewHooks();
  it("starts each pick with an empty composer and shows a failed post only in the banner", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async (url, init) => url.startsWith("/api/viewers/")
        ? new Response(JSON.stringify(viewer))
        : init?.method === "POST"
          ? new Response(JSON.stringify({ error: { code: "internal", message: "disk full" } }), { status: 500 })
          : new Response(JSON.stringify({ threads: [], next_cursor: null })));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    // A pick counts only in comment mode.
    viewerPick(frame, pick("p0", "Not in comment mode"));
    await new Promise(r => setTimeout(r, 30));
    expect(root.querySelector(".composer")).toBeNull();
    buttonNamed(root, "Comment").click();
    await waitFor(() => buttonNamed(root, "Comment").getAttribute("aria-pressed") === "true", "comment mode");
    viewerPick(frame, pick("p1", "Quarterly goals"));
    let textarea = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
    textarea.value = "first draft";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => postReady(root), "post enabled");
    buttonNamed(root, "Post comment").click();
    const banner = await waitFor(() => root.querySelector(".banner.notice"), "notice banner");
    expect(banner.textContent).toContain("Could not post: 500 disk full");
    expect(root.querySelector(".composer .error")).toBeNull();
    expect(root.querySelector(".composer")!.textContent).not.toContain("disk full");
    buttonNamed(root, "Comment").click();
    await waitFor(() => buttonNamed(root, "Comment").getAttribute("aria-pressed") === "true", "comment mode");
    viewerPick(frame, pick("p2", "Grow revenue"));
    await waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes("Grow revenue"), "second pick");
    textarea = root.querySelector<HTMLTextAreaElement>(".composer textarea")!;
    expect(textarea.value).toBe("");
  });

  it("opens no composer for a pick the page forged: without a start, with a start outside the viewer's gesture or without an anchor, or beside another pending start; and tells the bridge each start it refused", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    // Each message keeps what had focus when it was posted.
    const posted: { type: string; pickId?: string; focused?: Element | null }[] = [];
    win.postMessage = ((m: { type: string }) => { posted.push({ ...m, focused: document.activeElement }); }) as typeof win.postMessage;
    const refused = () => posted.filter(m => m.type === "clax:pick-refused").map(m => m.pickId);
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    const comment = buttonNamed(root, "Comment");
    comment.click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode");
    const settle = () => new Promise(r => setTimeout(r, 30));
    gestureIn(frame);
    fromFrame(win, pick("f1", "No start"));
    await settle();
    expect(root.querySelector(".composer")).toBeNull();
    gestureIn(frame, false);
    fromFrame(win, startOf(pick("f2", "No gesture")));
    fromFrame(win, pick("f2", "No gesture"));
    await settle();
    expect(root.querySelector(".composer")).toBeNull();
    gestureIn(frame);
    fromFrame(win, { type: "clax:pick-start", pickId: "f3" });
    fromFrame(win, pick("f3", "No anchor"));
    await settle();
    expect(root.querySelector(".composer")).toBeNull();
    expect(comment.getAttribute("aria-pressed")).toBe("true");
    expect(refused()).toEqual(["f2", "f3"]);
    // The page's forged start beside the bridge's real one: neither counts,
    // the composer the first opened closes, and comment mode comes back.
    gestureIn(frame);
    fromFrame(win, startOf(pick("real", "Real")));
    fromFrame(win, startOf(pick("forged", "Forged")));
    fromFrame(win, pick("forged", "Forged"));
    fromFrame(win, pick("real", "Real"));
    await settle();
    expect(root.querySelector(".composer")).toBeNull();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode back");
    expect(refused()).toEqual(["f2", "f3", "forged", "real"]);
    expect(posted.some(m => m.type === "clax:composer-ready")).toBe(false);
    // Beside a composer the viewer has typed in: that composer and its text stay.
    gestureIn(frame);
    fromFrame(win, startOf(pick("mine", "Mine")));
    const textarea = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "the composer");
    // Its textarea has focus: the bridge may render the clip now.
    await waitFor(() => posted.some(m => m.type === "clax:composer-ready" && m.pickId === "mine"), "composer ready");
    // It had focus before the message went.
    expect(posted.find(m => m.type === "clax:composer-ready")!.focused).toBe(textarea);
    expect(document.activeElement).toBe(textarea);
    textarea.value = "my words";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    gestureIn(frame);
    fromFrame(win, startOf(pick("other", "Other")));
    await settle();
    expect(root.querySelector(".composer-quote")!.textContent).toContain("Mine");
    expect(root.querySelector<HTMLTextAreaElement>(".composer textarea")!.value).toBe("my words");
    expect(refused().at(-1)).toBe("other");
    fromFrame(win, { ...pick("mine", "Mine"), clipError: "kept" });
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("No screenshot: kept"), "its screenshot still comes");
    buttonNamed(root, "Cancel").click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode after cancel");
    // A start is used once: a second pick under its ID changes nothing.
    viewerPick(frame, { ...pick("ok", "Quarterly goals"), clipError: "first" });
    await waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes("Quarterly goals"), "the viewer's pick");
    comment.click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode again");
    fromFrame(win, { ...pick("ok", "Replayed"), clipError: "replayed" });
    await settle();
    expect(root.querySelector(".composer-quote")!.textContent).toContain("Quarterly goals");
    expect(root.querySelector(".composer")!.textContent).toContain("No screenshot: first");
  });

  it("opens the composer at the viewer's pick start, focused and taking the screenshot, and takes no text from the page", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    const comment = buttonNamed(root, "Comment");
    comment.click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode");
    const settle = () => new Promise(r => setTimeout(r, 30));
    gestureIn(frame);
    fromFrame(win, startOf(pick("p1", "Goals")));
    const textarea = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "the composer");
    await waitFor(() => root.querySelector(".composer")!.textContent!.includes("Taking the screenshot…"), "the status line");
    expect(buttonNamed(root, "Post comment").disabled).toBe(true);
    expect(comment.getAttribute("aria-pressed")).toBe("false");
    await waitFor(() => document.activeElement === textarea, "the textarea focused");
    // Text the page posts goes nowhere.
    fromFrame(win, { type: "clax:keys", pickId: "p1", keys: ["@", "a", "g", "e", "n", "t"], done: true });
    await settle();
    expect(textarea.value).toBe("");
    // The screenshot arrives: the composer keeps its anchor; Post waits for text.
    fromFrame(win, pick("p1", "Ignored anchor"));
    await waitFor(() => !root.querySelector(".composer")!.textContent!.includes("Taking the screenshot…"), "the screenshot");
    expect(root.querySelector(".composer-quote")!.textContent).toContain("Goals");
    textarea.value = "Via";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => postReady(root), "post enabled");
    buttonNamed(root, "Cancel").click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode back");
  });

  it("tells a custom-anchors page areas are off while a send is in flight, and a page area's composer waits for its screenshot with Post disabled, then says it never came", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const thread = { id: "tS", artifact_id: ID, version_n: 1, anchor: pick("x", "Goals").anchor, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: "c1", thread_id: "tS", author_kind: "viewer", author_name: "Viewer", via_harness: null, body: "note", created_at: "x" }] };
    let answerSend!: (r: Response) => void;
    const declared = { ...artifact(1, { "index.html": page }), artifact: { ...artifact(1).artifact, capabilities: { comments: { customAnchors: true } } } };
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : declared)),
      async (url, init) => url.startsWith("/api/viewers/")
        ? new Response(JSON.stringify(viewer))
        : url.endsWith("/send") && init?.method === "POST"
          ? new Promise<Response>(r => { answerSend = r; })
          : new Response(JSON.stringify({ threads: [thread], next_cursor: null })));
    const root = view.root;
    const captureWait = (await import("./view/composer-model")).captureWait;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string; topic?: string; data?: { on?: boolean; canArea?: boolean }; ok?: boolean; value?: { opened?: boolean } }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    fromFrame(win, { type: "clax:call", id: "r1", ns: "comments", method: "register", args: [] });
    await waitFor(() => posted.some(m => m.type === "clax:call-result" && m.id === "r1"), "registered");
    buttonNamed(root, "Comment").click();
    const lastMode = () => posted.filter(m => m.type === "clax:event" && m.topic === "mode").at(-1)?.data;
    await waitFor(() => lastMode()?.on === true && lastMode()?.canArea === true, "areas on in comment mode");
    const card = await waitFor(() => root.querySelector('[data-thread="tS"]'), "the thread");
    // A pointer's click: every load starts with the keyboard trail tainted (`keyboardTrail`).
    pointerClick(buttonNamed(card, "Send to claude"));
    await waitFor(() => lastMode()?.canArea === false, "areas off while the send is in flight");
    answerSend(new Response(JSON.stringify({ thread: { ...thread, sent_to_agent: true } })));
    await waitFor(() => lastMode()?.canArea === true, "areas on again");
    // A page area: the composer opens at once, waiting for its screenshot.
    gestureIn(frame);
    fromFrame(win, { type: "clax:call", id: "c1", ns: "comments", method: "compose", args: [{ anchor: "body > h2", dom: true, area: true, clipPending: true, version: 1 }] });
    await waitFor(() => posted.find(m => m.type === "clax:call-result" && m.id === "c1"), "compose answered");
    expect(posted.find(m => m.id === "c1")!.value).toMatchObject({ opened: true });
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("Taking the screenshot…"), "capturing");
    const textarea = root.querySelector<HTMLTextAreaElement>(".composer textarea")!;
    textarea.value = "look here";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await new Promise(r => setTimeout(r, 0));
    expect(buttonNamed(root, "Post comment").getAttribute("aria-disabled")).toBe("true");
    // The screenshot's wait runs on the shell's clock: past it, the note.
    (globalThis as unknown as { claxTestClock: { advance(ms: number): void } }).claxTestClock.advance(captureWait.ms);
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("No screenshot: it was not taken in time"), "the late note");
    await waitFor(() => postReady(root), "post enabled");
  });

  it("drops a pick's clip past the daemon's cap with the reason, says when a thread was posted without its screenshot, and clears that on a post that kept its clip", async () => {
    let posts = 0;
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async (url, init) => url.startsWith("/api/viewers/")
        ? new Response(JSON.stringify(viewer))
        : init?.method === "POST"
          ? new Response(JSON.stringify({ thread: { id: `01JX${++posts}`, artifact_id: ID, version_n: 1, anchor: pick("x", "q").anchor, status: "open", sent_to_agent: false, has_clip: posts > 1, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null, comments: [] }, ...(posts === 1 ? { clip_error: "clip is not a PNG" } : {}) }), { status: 201 })
          : new Response(JSON.stringify({ threads: [], next_cursor: null })));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    fromFrame(frame.contentWindow!, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    buttonNamed(root, "Comment").click();
    await waitFor(() => buttonNamed(root, "Comment").getAttribute("aria-pressed") === "true", "comment mode");
    viewerPick(frame, { ...pick("big", "Photo"), clipPng: new ArrayBuffer(5 * 1024 * 1024 + 1) });
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("No screenshot: the screenshot was too large to keep"), "the reason");
    const textarea = root.querySelector<HTMLTextAreaElement>(".composer textarea")!;
    textarea.value = "look";
    textarea.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => postReady(root), "post enabled");
    buttonNamed(root, "Post comment").click();
    await waitFor(() => root.querySelector(".banner.notice")?.textContent?.includes("Posted without its screenshot: clip is not a PNG"), "the notice");
    // The next post keeps its clip: that notice goes (jsdom has no object URLs for its preview).
    const urls = URL as unknown as { createObjectURL?: unknown; revokeObjectURL?: unknown };
    const had = { create: urls.createObjectURL, revoke: urls.revokeObjectURL };
    urls.createObjectURL = () => "blob:clip";
    urls.revokeObjectURL = () => {};
    onTestFinished(() => { urls.createObjectURL = had.create; urls.revokeObjectURL = had.revoke; });
    await waitFor(() => buttonNamed(root, "Comment").getAttribute("aria-pressed") === "true", "comment mode again");
    viewerPick(frame, { ...pick("small", "Chart"), clipPng: new Uint8Array([137, 80, 78, 71]).buffer });
    const t2 = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "second composer");
    t2.value = "and this";
    t2.dispatchEvent(new Event("input", { bubbles: true }));
    await waitFor(() => postReady(root), "post enabled");
    buttonNamed(root, "Post comment").click();
    await waitFor(() => posts === 2 && !root.querySelector(".banner.notice"), "the notice cleared");
  });

  it("takes a pick's screenshot only for the composer its start opened, once, and forgets a pending pick when comment mode comes back on or a page greets, so the viewer's next pick works", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const hello = () => fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    hello();
    const comment = buttonNamed(root, "Comment");
    const on = async () => { if (comment.getAttribute("aria-pressed") !== "true") comment.click(); await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode on"); };
    const settle = () => new Promise(r => setTimeout(r, 30));
    const start = (id: string, quote: string) => { gestureIn(frame); fromFrame(win, startOf(pick(id, quote))); };
    const capturing = () => !!root.querySelector(".composer")?.textContent?.includes("Taking the screenshot…");
    const composerFor = (quote: string) => waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes(quote) && capturing(), quote);
    // The start opens the composer; its pick brings the screenshot, once.
    await on();
    start("a1", "First");
    await composerFor("First");
    fromFrame(win, { ...pick("a1", "First"), clipError: "blank" });
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("No screenshot: blank"), "the pick's clip error");
    fromFrame(win, { ...pick("a1", "First"), clipError: "again" });
    await settle();
    expect(root.querySelector(".composer")!.textContent).toContain("No screenshot: blank");
    // Comment mode back on while a pick is in flight: the old pick is moot,
    // the new one works.
    await on();
    start("b1", "Old");
    await composerFor("Old");
    await on();
    start("b2", "New after a toggle");
    await composerFor("New after a toggle");
    fromFrame(win, { ...pick("b1", "Old"), clipError: "old" });
    await settle();
    expect(capturing()).toBe(true);
    fromFrame(win, { ...pick("b2", "New after a toggle"), clipError: "new" });
    await waitFor(() => root.querySelector(".composer")?.textContent?.includes("No screenshot: new"), "the new pick's clip");
    // A pick from before a new greeting does not count after it.
    await on();
    start("c1", "Before the greeting");
    await composerFor("Before the greeting");
    hello();
    fromFrame(win, { ...pick("c1", "Before the greeting"), clipError: "stale" });
    await settle();
    expect(capturing()).toBe(true);
    await on();
    viewerPick(frame, { ...pick("c2", "After the greeting"), clipError: "fresh" });
    await waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes("After the greeting") && root.querySelector(".composer")?.textContent?.includes("No screenshot: fresh"), "after the greeting");
  });

  it("turns comment mode back on when a pick's composer closes by Post, by a post sent to the agent, or by Cancel or Escape, and not while a failed post keeps it open", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    let posts = 0;
    const bodies: string[] = [];
    let sends = 0;
    const thread = (id: string, body: string) => ({ id, artifact_id: ID, version_n: 1, anchor: pick("x", "q").anchor, status: "open", sent_to_agent: body.includes("@agent"), has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "Viewer", via_harness: null, body, created_at: "x" }] });
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async (url, init) => {
        if (url.startsWith("/api/viewers/")) return new Response(JSON.stringify(viewer));
        if (url.endsWith("/send") && init?.method === "POST") { sends++; return new Response(JSON.stringify({ thread: { ...thread("t1", "second"), sent_to_agent: true } })); }
        if (init?.method === "POST") {
          const body = String((init.body as FormData).get("body"));
          bodies.push(body);
          if (++posts === 1) return new Response(JSON.stringify({ error: { code: "internal", message: "disk full" } }), { status: 500 });
          return new Response(JSON.stringify({ thread: thread(`t${posts - 1}`, body) }), { status: 201 });
        }
        return new Response(JSON.stringify({ threads: [], next_cursor: null }));
      });
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const toFrame: { type: string; on?: boolean }[] = [];
    win.postMessage = ((m: (typeof toFrame)[number]) => { toFrame.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    const comment = buttonNamed(root, "Comment");
    const pressed = () => comment.getAttribute("aria-pressed") === "true";
    const lastMode = () => toFrame.filter(m => m.type === "clax:comment-mode").at(-1)?.on;
    const settle = () => new Promise(r => setTimeout(r, 30));
    const type = async (text: string) => {
      const t = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
      t.value = text;
      t.dispatchEvent(new Event("input", { bubbles: true }));
      await waitFor(() => postReady(root), "post enabled");
    };
    comment.click();
    await waitFor(pressed, "comment mode");
    // Off while the composer is open, and still off after a failed post.
    viewerPick(frame, pick("p1", "Goals"));
    await type("first");
    expect(pressed()).toBe(false);
    // The frame hears of it from an effect, which runs after the render.
    await waitFor(() => lastMode() === false, "comment mode off in the frame");
    buttonNamed(root, "Post comment").click();
    await waitFor(() => root.querySelector(".banner.notice"), "the failed post");
    await settle();
    expect(root.querySelector(".composer")).not.toBeNull();
    expect(pressed()).toBe(false);
    expect(lastMode()).toBe(false);
    // Cancel closes it: comment mode is back on, in the shell and the frame.
    buttonNamed(root, "Cancel").click();
    await waitFor(() => pressed() && lastMode() === true, "comment mode back after Cancel");
    // The next pick needs no press of Comment; Post closes it and mode is back on.
    viewerPick(frame, pick("p2", "Revenue"));
    await type("second");
    expect(pressed()).toBe(false);
    buttonNamed(root, "Post comment").click();
    await waitFor(() => !root.querySelector(".composer") && pressed() && lastMode() === true, "comment mode back after Post");
    // Sending the posted thread to the agent from its card leaves it on.
    // The viewer's press in the shell clears the keyboard trail that the
    // composer's focus tainted (`keyboardTrail`).
    (await import("./view/trail")).keyboardTrail.clear();
    buttonNamed(await waitFor(() => root.querySelector('[data-thread="t1"]'), "the posted thread"), "Send to claude").click();
    await waitFor(() => sends === 1, "the send");
    await settle();
    expect(pressed()).toBe(true);
    // A post that sends itself to the agent (`@agent`) closes it too.
    viewerPick(frame, pick("p3", "Costs"));
    await type("@agent third");
    buttonNamed(root, "Post comment").click();
    await waitFor(() => !root.querySelector(".composer") && pressed(), "comment mode back after a post to the agent");
    expect(bodies).toEqual(["first", "second", "@agent third"]);
    // Escape in the composer cancels it: back on.
    viewerPick(frame, pick("p4", "Hiring"));
    const t4 = await waitFor(() => root.querySelector<HTMLTextAreaElement>(".composer textarea"), "composer");
    expect(pressed()).toBe(false);
    t4.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await waitFor(() => !root.querySelector(".composer") && pressed(), "comment mode back after Escape in the composer");
    // Escape with no composer open still ends comment mode, in the shell and from the page.
    root.querySelector("header")!.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await waitFor(() => !pressed() && lastMode() === false, "comment mode off on Escape");
    comment.click();
    await waitFor(pressed, "comment mode on");
    fromFrame(win, { type: "clax:cancel" });
    await waitFor(() => !pressed(), "comment mode off on the page's cancel");
  });

  it("leaves comment mode off when a composer closes that no pick in comment mode opened, or that the viewer turned mode on and off over", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = { id: "tR", artifact_id: ID, version_n: 1, anchor: pick("x", "Goals").anchor, status: "open", sent_to_agent: true, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: "c1", thread_id: "tR", author_kind: "viewer", author_name: "Viewer", via_harness: null, body: "note", created_at: "x" }] };
    let replies = 0;
    const declared = { ...artifact(1, { "index.html": page }), artifact: { ...artifact(1).artifact, capabilities: { comments: {} } } };
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : declared)),
      async (url, init) => {
        if (url.startsWith("/api/viewers/")) return new Response(JSON.stringify(viewer));
        if (url.includes("/comments") && init?.method === "POST") { replies++; return new Response(JSON.stringify({ thread: t }), { status: 201 }); }
        return new Response(JSON.stringify({ threads: [t], next_cursor: null }));
      });
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string; value?: { opened?: boolean } }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    const comment = buttonNamed(root, "Comment");
    const pressed = () => comment.getAttribute("aria-pressed") === "true";
    const settle = () => new Promise(r => setTimeout(r, 30));
    const pageOpens = async (id: string) => {
      gestureIn(frame);
      fromFrame(win, { type: "clax:call", id, ns: "comments", method: "openComposer", args: [{ anchor: pick("x", "Goals").anchor }] });
      await waitFor(() => posted.find(m => m.type === "clax:call-result" && m.id === id)?.value?.opened, `the page's composer ${id}`);
    };
    const cancelStaysOff = async () => {
      buttonNamed(root, "Cancel").click();
      await waitFor(() => !root.querySelector(".composer"), "composer closed");
      await settle();
      expect(pressed()).toBe(false);
    };
    // The page's openComposer, then Cancel: mode stays off.
    await pageOpens("o1");
    // The page opened it: text typed into it never posts from the keyboard.
    const ta = root.querySelector<HTMLTextAreaElement>(".composer textarea")!;
    ta.value = "typed for the page";
    ta.dispatchEvent(new Event("input", { bubbles: true }));
    ta.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", metaKey: true, bubbles: true, cancelable: true }));
    await settle();
    expect(root.querySelector(".composer .act-hint")?.textContent).toBe("Click to post");
    await cancelStaysOff();
    // A pick's empty composer that the page's openComposer replaced: closing it leaves mode off.
    comment.click();
    await waitFor(pressed, "comment mode");
    viewerPick(frame, pick("p1", "Picked"));
    await waitFor(() => root.querySelector(".composer-quote")?.textContent?.includes("Picked"), "the pick's composer");
    await pageOpens("o2");
    await cancelStaysOff();
    // The viewer turned mode on and off again over a pick's composer: it stays off.
    comment.click();
    await waitFor(pressed, "comment mode");
    viewerPick(frame, pick("p2", "Picked again"));
    await waitFor(() => root.querySelector(".composer") && !pressed(), "the pick's composer");
    comment.click();
    await waitFor(pressed, "on over the composer");
    comment.click();
    await waitFor(() => !pressed(), "off over the composer");
    await cancelStaysOff();
    // A reply on a thread card does not turn it on.
    const card = await waitFor(() => root.querySelector('[data-thread="tR"]'), "the thread");
    const input = card.querySelector<HTMLInputElement>("input[aria-label=Reply]")!;
    input.value = "more";
    input.dispatchEvent(new Event("input", { bubbles: true }));
    await settle();
    // The viewer's press in the shell clears the keyboard trail every load starts with.
    (await import("./view/trail")).keyboardTrail.clear();
    buttonNamed(card, "Reply").click();
    await waitFor(() => replies === 1, "the reply");
    await settle();
    expect(pressed()).toBe(false);
  });

  it("tells a custom-anchors page comment mode came back, with areas, when a pick's composer closes", async () => {
    const declared = { ...artifact(1, { "index.html": page }), artifact: { ...artifact(1).artifact, capabilities: { comments: { customAnchors: true } } } };
    const view = await mountView(async url => new Response(JSON.stringify(url === "/api/token" ? { token: "tk" } : declared)));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string; topic?: string; data?: { on?: boolean; canArea?: boolean } }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    fromFrame(win, { type: "clax:call", id: "r1", ns: "comments", method: "register", args: [] });
    await waitFor(() => posted.some(m => m.type === "clax:call-result" && m.id === "r1"), "registered");
    const modes = () => posted.filter(m => m.type === "clax:event" && m.topic === "mode").map(m => m.data);
    buttonNamed(root, "Comment").click();
    await waitFor(() => modes().at(-1)?.on === true, "mode on");
    viewerPick(frame, pick("p1", "Goals"));
    await waitFor(() => modes().at(-1)?.on === false, "mode off while composing");
    buttonNamed(root, "Cancel").click();
    await waitFor(() => modes().at(-1)?.on === true, "mode back on");
    expect(modes()).toEqual([{ on: false, canArea: false }, { on: true, canArea: true }, { on: false, canArea: false }, { on: true, canArea: true }]);
  });

  it("sends Escape to the frame while commenting with the pointer over it, and leaves comment mode only when the page answers", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; key?: string; down?: boolean }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    const comment = buttonNamed(root, "Comment");
    comment.click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode");
    frame.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    expect(posted.filter(m => m.type === "clax:key").at(-1)).toEqual({ type: "clax:key", key: "Escape", down: true });
    await new Promise(r => setTimeout(r, 30));
    // The page dropped a drag: comment mode stays on.
    expect(comment.getAttribute("aria-pressed")).toBe("true");
    fromFrame(win, { type: "clax:cancel" });
    await waitFor(() => comment.getAttribute("aria-pressed") === "false", "comment mode off on the page's answer");
    // Away from the frame, Escape ends comment mode in the shell.
    comment.click();
    await waitFor(() => comment.getAttribute("aria-pressed") === "true", "comment mode again");
    root.querySelector("header")!.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    const keys = posted.filter(m => m.type === "clax:key").length;
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }));
    await waitFor(() => comment.getAttribute("aria-pressed") === "false", "comment mode off");
    expect(posted.filter(m => m.type === "clax:key")).toHaveLength(keys);
  });

  it("focuses the frame on the thread hovered in the list or selected, by its anchor handle, so its drawn area is outlined", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = { id: "tZ", artifact_id: ID, version_n: 1, anchor: { kind: "area", selector: "main", quote: null, prefix: null, suffix: null, html_hash: null, rect: null, custom_name: null, area: { x: 0, y: 0, w: 0.5, h: 0.5 }, file: "index.html" },
      status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: "c1", thread_id: "tZ", author_kind: "viewer", author_name: "Viewer", via_harness: null, body: "gap", created_at: "x" }] };
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t], next_cursor: null } : viewer)));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; id?: string | null; anchors?: { id: string }[] }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    const card = await waitFor(() => root.querySelector('[data-thread="tZ"]'), "the card");
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    // The frame knows the thread only by the opaque handle it was sent.
    const first = await waitFor(() => posted.filter(m => m.type === "clax:resolve-anchors").at(-1)?.anchors?.[0], "the resolve request");
    // Made on the version shown: the frame skips the area's fingerprint.
    expect((first as { sameVersion?: boolean }).sameVersion).toBe(true);
    const handle = first.id;
    expect(handle).not.toBe("tZ");
    const lastFocus = () => posted.filter(m => m.type === "clax:focus").at(-1);
    expect(lastFocus()).toEqual({ type: "clax:focus", id: null });
    card.dispatchEvent(new MouseEvent("mouseenter"));
    await waitFor(() => lastFocus()?.id === handle, "focus on the hovered card");
    card.dispatchEvent(new MouseEvent("mouseleave"));
    await waitFor(() => lastFocus()?.id === null, "focus cleared");
    card.querySelector<HTMLButtonElement>("button.card-head")!.click();
    await waitFor(() => lastFocus()?.id === handle, "focus on the selected thread");
    expect(posted.some(m => m.type === "clax:focus" && m.id === "tZ")).toBe(false);
  });

  it("tells the frame which threads were made on the version shown, to resolve and to scroll to", async () => {
    stubMedia({ "(min-width: 900px)": true, "(max-width: 480px)": false });
    const t = (id: string, n: number) => ({ id, artifact_id: ID, version_n: n, anchor: { ...pick("x", `Goals ${id}`).anchor }, status: "open", sent_to_agent: false, has_clip: false, clip_url: null, created_at: "x", resolved_at: null, resolved_by: null, feedback_state: null,
      comments: [{ id: `c${id}`, thread_id: id, author_kind: "viewer", author_name: "Viewer", via_harness: null, body: `note ${id}`, created_at: "x" }] });
    const two = { artifact: artifact(2).artifact, versions: [...artifact(1).versions, ...artifact(2).versions] };
    const view = await mountView(async () => new Response(JSON.stringify(two)),
      async url => new Response(JSON.stringify(url.includes("/threads") ? { threads: [t("tOld", 1), t("tNew", 2)], next_cursor: null } : viewer)));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; anchors?: { id: string; sameVersion?: boolean }[]; anchor?: { quote: string | null }; sameVersion?: boolean }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    await waitFor(() => root.querySelector('[data-thread="tOld"]'), "threads");
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 2, file: "index.html" });
    const req = await waitFor(() => posted.filter(m => m.type === "clax:resolve-anchors").at(-1)?.anchors?.length === 2 && posted.filter(m => m.type === "clax:resolve-anchors").at(-1), "the resolve request");
    expect(req.anchors!.map(a => a.sameVersion)).toEqual([false, true]);
    for (const [id, same] of [["tOld", false], ["tNew", true]] as const) {
      root.querySelector<HTMLButtonElement>(`[data-thread="${id}"] button.card-head`)!.click();
      const scroll = await waitFor(() => posted.filter(m => m.type === "clax:scroll-to").at(-1)?.anchor?.quote === `Goals ${id}` && posted.filter(m => m.type === "clax:scroll-to").at(-1), `scroll to ${id}`);
      expect(scroll.sameVersion).toBe(same);
    }
  });

  it("forwards Option and, with it, Up and Down to the frame while commenting with the pointer over it", async () => {
    const view = await mountView(async () => new Response(JSON.stringify(artifact(1))));
    const root = view.root;
    const frame = await waitFor(() => root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    const posted: { type: string; key?: string; down?: boolean }[] = [];
    win.postMessage = ((m: (typeof posted)[number]) => { posted.push(m); }) as typeof win.postMessage;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    await waitFor(() => posted.some(m => m.type === "clax:welcome"), "welcome");
    const keys = () => posted.filter(m => m.type === "clax:key").map(m => `${m.key}:${m.down}`);
    const press = (key: string, type = "keydown", altKey = false) => { const e = new KeyboardEvent(type, { key, altKey, bubbles: true, cancelable: true }); document.body.dispatchEvent(e); return e; };
    frame.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    press("Alt");
    expect(keys()).toEqual([]);
    buttonNamed(root, "Comment").click();
    await waitFor(() => posted.some(m => m.type === "clax:comment-mode"), "comment mode on");
    press("Alt");
    expect(press("ArrowUp", "keydown", true).defaultPrevented).toBe(true);
    press("ArrowDown", "keydown", true);
    expect(press("ArrowUp").defaultPrevented).toBe(false);
    press("Alt", "keyup");
    expect(keys()).toEqual(["Alt:true", "ArrowUp:true", "ArrowDown:true", "Alt:false"]);
    // The pointer left the frame: nothing more is forwarded.
    root.querySelector("header")!.dispatchEvent(new MouseEvent("mouseover", { bubbles: true }));
    press("Alt");
    expect(keys()).toHaveLength(4);
  });
});
