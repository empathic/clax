import { describe, expect, it } from "vitest";
import { dispatchTrusted } from "../../bridge/test/trusted";
import { MOUNT_TIMEOUT_MS } from "./test/timeouts";
import { ID, artifact, artifactViewHooks, fromFrame, gesture, gestureIn, mountView, waitFor } from "./test/artifact-view";

// C pressed in the page (`clax:shortcut`, sent by the bridge): the shell's own
// C, through `shortcut`, with its rules and the frame's gesture.
describe("C pressed in the page", { timeout: MOUNT_TIMEOUT_MS }, () => {
  artifactViewHooks();

  async function open(body = artifact(1)) {
    const view = await mountView(async () => new Response(JSON.stringify(body)));
    const frame = await waitFor(() => view.root.querySelector<HTMLIFrameElement>("iframe.frame"), "viewer");
    const win = frame.contentWindow!;
    fromFrame(win, { type: "clax:hello", artifact: ID, version: 1, file: "index.html" });
    const commenting = () => view.controller().state.get().commenting;
    return { view, frame, win, commenting };
  }
  const C = { type: "clax:shortcut", key: "c" };

  it("toggles comment mode as the shell's C does, only with the viewer's gesture in the frame and only for C", async () => {
    const { view, frame, win, commenting } = await open();
    gestureIn(frame);
    fromFrame(win, C);
    expect(commenting()).toBe(true);
    gestureIn(frame);
    fromFrame(win, C);
    expect(commenting()).toBe(false);
    // The page posting it with no gesture of the viewer's in the frame.
    gestureIn(frame, false);
    fromFrame(win, C);
    expect(commenting()).toBe(false);
    // Any other key it names.
    gestureIn(frame);
    for (const key of ["C", "?", "t", 67, undefined]) fromFrame(win, { type: "clax:shortcut", key });
    expect(commenting()).toBe(false);
    // Nor while the keys sheet is open.
    expect(view.controller().shortcut("help")).toBe(true);
    gestureIn(frame);
    fromFrame(win, C);
    expect(commenting()).toBe(false);
    view.controller().closeSheet();
    gestureIn(frame);
    fromFrame(win, C);
    expect(commenting()).toBe(true);
  });

  it("does nothing where the Comment button is off: comments turned off for the page", async () => {
    const base = artifact(1);
    const { frame, win, commenting } = await open({ ...base, artifact: { ...base.artifact, comments: false } as typeof base.artifact });
    gestureIn(frame);
    fromFrame(win, C);
    expect(commenting()).toBe(false);
  });

  it("is held after a load the page caused until the viewer presses in the shell", async () => {
    sessionStorage.setItem("clax.keys-held", "1");
    const { view, frame, win, commenting } = await open();
    // The viewer's gesture in the frame, with no focus on a shell control
    // first (that would give the keys back, as the press below does).
    Object.defineProperty(navigator, "userActivation", { value: { isActive: true }, configurable: true });
    gesture!.notePointerOver(frame, 100, 100);
    frame.focus();
    expect(gesture!.frameGesture()).toBe(true);
    fromFrame(win, C);
    expect(commenting()).toBe(false);
    const h1 = await waitFor(() => view.root.querySelector<HTMLElement>(".topbar h1"), "the title");
    dispatchTrusted(h1, new MouseEvent("pointerdown", { bubbles: true, clientX: 900, clientY: 10 }));
    gestureIn(frame);
    expect(gesture!.frameGesture()).toBe(true);
    fromFrame(win, C);
    expect(commenting()).toBe(true);
  });
});
