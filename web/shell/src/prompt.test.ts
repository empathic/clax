import { afterEach, describe, expect, it, vi } from "vitest";
import { ALLOW_DELAY_MS, type Ask, PromptDialog } from "./prompt";
import { flush, mount } from "./test/preact";

const button = (root: Element, name: string) => Array.from(root.querySelectorAll("button")).find(b => b.textContent === name)!;

function mountDialog() {
  const answer = vi.fn();
  const ask: Ask = { prompt: { title: "Allow this page to act as you?", body: "b", allow: "Allow", deny: "Don't allow" }, answer };
  const { root } = mount(PromptDialog, { ask });
  return { root, answer };
}

describe("PromptDialog", () => {
  afterEach(() => { vi.useRealTimers(); document.body.replaceChildren(); });

  it("opens with focus on Don't allow and keeps Allow inert for the first 500 ms", () => {
    vi.useFakeTimers();
    const { root, answer } = mountDialog();
    expect(ALLOW_DELAY_MS).toBe(500);
    expect(document.activeElement).toBe(button(root, "Don't allow"));
    const allow = button(root, "Allow");
    expect(allow.disabled).toBe(true);
    allow.click();
    flush(() => { vi.advanceTimersByTime(ALLOW_DELAY_MS - 1); });
    expect(button(root, "Allow").disabled).toBe(true);
    expect(answer).not.toHaveBeenCalled();
    flush(() => { vi.advanceTimersByTime(1); });
    expect(button(root, "Allow").disabled).toBe(false);
    button(root, "Allow").click();
    expect(answer).toHaveBeenCalledWith("allow");
  });

  it("dismisses on Escape at once, and denies on Don't allow", () => {
    vi.useFakeTimers();
    const first = mountDialog();
    dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    expect(first.answer).toHaveBeenCalledWith("dismiss");
    document.body.replaceChildren();
    const second = mountDialog();
    button(second.root, "Don't allow").click();
    expect(second.answer).toHaveBeenCalledWith("deny");
  });
});
