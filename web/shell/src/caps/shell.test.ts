import { afterEach, describe, expect, it, vi } from "vitest";
import { showCoachMark } from "../ui/coach-mark";
import type { ShellControl } from "../ui/shell-controls";
import type { Grants } from "./grants";
import type { CapEnv, Handler } from "./host";
import { type ShellDeps, makeShellHandler, shellAddress } from "./shell";

const button = () => {
  const el = document.createElement("button");
  document.body.appendChild(el);
  return el;
};

/** A handler over fake deps: `find` answers from `shown`, `mark` records the
 * element it rings and returns a dismiss spy. */
function setup(shown: Partial<Record<ShellControl, HTMLElement>> = {}, commentMode: (on: boolean) => boolean | null = on => on, origin = "https://clax.dev") {
  const marked: HTMLElement[] = [];
  const dismiss = vi.fn();
  const deps: ShellDeps = { find: c => shown[c] ?? null, mark: el => { marked.push(el); return dismiss; }, origin: () => origin };
  const env = { aid: "abcd1234efgh", commentMode: vi.fn(commentMode) } as unknown as CapEnv & { commentMode: ReturnType<typeof vi.fn> };
  const h = makeShellHandler(deps)(env, {} as Grants);
  return { h, env, marked, dismiss };
}

const ok = { composer: true };

afterEach(() => { document.body.replaceChildren(); });

describe("the shell capability", () => {
  it("highlight_rings_a_shown_control_and_resolves_true", async () => {
    const el = button();
    const { h, marked } = setup({ comment: el });
    await expect(h.call("highlight", ["comment"])).resolves.toBe(true);
    expect(marked).toEqual([el]);
  });

  it("highlight_of_a_control_not_shown_draws_nothing", async () => {
    const { h, marked } = setup();
    await expect(h.call("highlight", ["fork"])).resolves.toBe(false);
    expect(marked).toEqual([]);
  });

  it("highlight_of_a_control_not_shown_clears_the_previous_ring", async () => {
    const { h, dismiss } = setup({ comment: button() });
    await expect(h.call("highlight", ["comment"])).resolves.toBe(true);
    expect(dismiss).not.toHaveBeenCalled();
    await expect(h.call("highlight", ["fork"])).resolves.toBe(false);
    expect(dismiss).toHaveBeenCalledTimes(1);
  });

  it("highlight_resolves_false_when_the_ring_could_not_be_drawn", async () => {
    const el = button();
    const h = makeShellHandler({ find: () => el, mark: () => null, origin: () => "" })({} as CapEnv, {} as Grants);
    await expect(h.call("highlight", ["comment"])).resolves.toBe(false);
    // The real mark: a control that is not in view at once draws nothing.
    const hidden = makeShellHandler({ find: () => el, mark: e => showCoachMark(e, () => false), origin: () => "" })({} as CapEnv, {} as Grants);
    await expect(hidden.call("highlight", ["comment"])).resolves.toBe(false);
    expect(document.querySelectorAll(".coach-mark")).toHaveLength(0);
    document.head.querySelector("style[data-coach-mark]")?.remove();
  });

  it("can_says_whether_shown_and_draws_nothing", async () => {
    const { h, marked } = setup({ threads: button() });
    await expect(h.call("can", ["threads"])).resolves.toBe(true);
    await expect(h.call("can", ["publish"])).resolves.toBe(false);
    expect(marked).toEqual([]);
  });

  it("an_unknown_control_or_a_non_boolean_is_invalid_argument", async () => {
    const { h, env, marked } = setup({ comment: button() });
    await expect(h.call("highlight", ["menu"])).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(h.call("can", [])).rejects.toMatchObject({ code: "invalid_argument" });
    await expect(h.call("commentMode", ["yes"], ok)).rejects.toMatchObject({ code: "invalid_argument" });
    expect(marked).toEqual([]);
    expect(env.commentMode).not.toHaveBeenCalled();
  });

  it("comment_mode_needs_the_viewers_gesture", async () => {
    const { h, env } = setup();
    await expect(h.call("commentMode", [true], { composer: false })).rejects.toMatchObject({ code: "no_gesture" });
    expect(env.commentMode).not.toHaveBeenCalled();
    await expect(h.call("commentMode", [true], ok)).resolves.toBe(true);
    expect(env.commentMode).toHaveBeenCalledWith(true);
    // Leaving keeps the mode when the view says so (a typed comment).
    env.commentMode.mockReturnValueOnce(true);
    await expect(h.call("commentMode", [false], ok)).resolves.toBe(true);
    expect(env.commentMode).toHaveBeenLastCalledWith(false);
  });

  it("comment_mode_where_the_view_cannot_comment_is_unavailable", async () => {
    const { h } = setup({}, () => null);
    await expect(h.call("commentMode", [true], ok)).rejects.toMatchObject({ code: "unavailable" });
    const without = makeShellHandler({ find: () => null, mark: () => () => {}, origin: () => "" })({} as CapEnv, {} as Grants);
    await expect(without.call("commentMode", [true], ok)).rejects.toMatchObject({ code: "unavailable" });
  });

  it("leaving_the_document_dismisses_the_mark", async () => {
    for (const end of ["leave", "reset", "dispose"] as const) {
      const { h, dismiss } = setup({ comment: button() });
      await h.call("highlight", ["comment"]);
      expect(dismiss).not.toHaveBeenCalled();
      (h[end] as NonNullable<Handler[typeof end]>)();
      expect(dismiss, end).toHaveBeenCalledTimes(1);
    }
  });

  it("a_hundred_highlights_leave_one_ring", async () => {
    const el = button();
    el.getClientRects = () => [{}] as unknown as DOMRectList;
    el.checkVisibility = () => true;
    el.getBoundingClientRect = () => ({ x: 10, y: 10, left: 10, top: 10, width: 50, height: 20, right: 60, bottom: 30, toJSON() {} }) as DOMRect;
    const h = makeShellHandler({ find: () => el, mark: e => showCoachMark(e), origin: () => "" })({} as CapEnv, {} as Grants);
    try {
      for (let i = 0; i < 100; i++) await expect(h.call("highlight", ["comment"])).resolves.toBe(true);
      expect(document.querySelectorAll(".coach-mark")).toHaveLength(1);
    } finally {
      h.dispose?.();
      document.head.querySelector("style[data-coach-mark]")?.remove();
    }
    expect(document.querySelectorAll(".coach-mark")).toHaveLength(0);
  });

  it("address_is_the_shells_origin_and_the_artifacts_latest_address_there", async () => {
    const { h, marked } = setup();
    await expect(h.call("address", [])).resolves.toEqual({ origin: "https://clax.dev", url: "https://clax.dev/a/abcd1234efgh" });
    // A local daemon's shell, on a port.
    const local = setup({}, on => on, "http://localhost:7480");
    await expect(local.h.call("address", [])).resolves.toEqual({ origin: "http://localhost:7480", url: "http://localhost:7480/a/abcd1234efgh" });
    expect(marked).toEqual([]);
  });

  it("address_is_null_off_the_web", async () => {
    // The Clax app's shell document is on its own scheme, which no agent can open.
    for (const o of ["clax://app", "null", "", "file://"]) expect(shellAddress(o, "abcd1234efgh"), o).toBeNull();
    const { h } = setup({}, on => on, "clax://app");
    await expect(h.call("address", [])).resolves.toBeNull();
  });

  it("the_real_handler_answers_with_this_windows_origin", async () => {
    const { shellHandler } = await import("./shell");
    const h = shellHandler({ aid: "abcd1234efgh" } as CapEnv, {} as Grants);
    await expect(h.call("address", [])).resolves.toEqual({ origin: location.origin, url: `${location.origin}/a/abcd1234efgh` });
  });

  it("an_unknown_method_is_capability_removed", async () => {
    const { h } = setup();
    await expect(h.call("open", ["comment"], ok)).rejects.toMatchObject({ code: "capability_removed" });
  });
});
