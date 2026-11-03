import { afterEach, describe, expect, it, vi } from "vitest";
import { OVERLAY_TAG } from "../../bridge/src/anchor";
import { overlay } from "./main";

const opts = { instance: "https://clax.example", queue: "q1", auth: async () => { throw new Error("no backend"); } };

afterEach(() => { for (const el of document.querySelectorAll(OVERLAY_TAG)) el.remove(); });

describe("overlay()", () => {
  it("refuses an instance that is not an origin, and adds nothing to the page", () => {
    expect(() => overlay({ ...opts, instance: "https://apps.example.com/clax" })).toThrow(/^Clax\.overlay: instance must be an origin/);
    expect(document.querySelectorAll(OVERLAY_TAG)).toHaveLength(0);
  });

  it("refuses an onShowReports that is not a function and a reportsLabel that is not a non-empty string, adding nothing to the page", () => {
    expect(() => overlay({ ...opts, onShowReports: "/feedback" as never })).toThrow(new TypeError("Clax.overlay: onShowReports must be a function"));
    for (const reportsLabel of ["", "  ", 7]) expect(() => overlay({ ...opts, reportsLabel: reportsLabel as never })).toThrow(new TypeError("Clax.overlay: reportsLabel must be a non-empty string"));
    expect(document.querySelectorAll(OVERLAY_TAG)).toHaveLength(0);
  });

  it("logs an onShowReports that throws, and the launcher goes on working", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    let asked = 0;
    const handle = overlay({ ...opts, onShowReports: () => { asked++; throw new Error("no route"); } });
    try {
      await new Promise(r => setTimeout(r, 0));
      const btn = document.querySelector(OVERLAY_TAG)?.shadowRoot?.querySelector<HTMLButtonElement>(".reports-btn");
      if (!btn) throw new Error("no Your reports button");
      btn.click();
      btn.click();
      expect(asked).toBe(2);
      expect(errors.mock.calls.filter(([m]) => m === "Clax.overlay: onShowReports threw")).toHaveLength(2);
    } finally {
      handle.close();
      errors.mockRestore();
    }
  });

  // As a React app in StrictMode does in development: its effect's setup,
  // cleanup and setup again, in one task.
  it("leaves nothing in the page when closed in the task that started it", () => {
    overlay(opts).close();
    expect(document.querySelectorAll(OVERLAY_TAG)).toHaveLength(0);
  });

  it("leaves nothing in the page when closed after its effects ran", async () => {
    const handle = overlay(opts);
    await new Promise(r => setTimeout(r, 0));
    expect(document.querySelectorAll(OVERLAY_TAG).length).toBeGreaterThan(0);
    handle.close();
    expect(document.querySelectorAll(OVERLAY_TAG)).toHaveLength(0);
  });
});
