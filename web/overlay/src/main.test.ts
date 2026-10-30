import { afterEach, describe, expect, it } from "vitest";
import { OVERLAY_TAG } from "../../bridge/src/anchor";
import { overlay } from "./main";

const opts = { instance: "https://clax.example", queue: "q1", auth: async () => { throw new Error("no backend"); } };

afterEach(() => { for (const el of document.querySelectorAll(OVERLAY_TAG)) el.remove(); });

describe("overlay()", () => {
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
