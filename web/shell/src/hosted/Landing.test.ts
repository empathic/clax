import { afterEach, describe, expect, it, vi } from "vitest";
import { mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import Landing from "./Landing.svelte";

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); document.body.replaceChildren(); });

const me = (signed_in: boolean) => ({ origin: "https://clax.example", setup_open: false, signed_in, person: null });

async function landing(signedIn: boolean) {
  stubFetch({ "GET /api/hosted/me": me(signedIn) });
  const m = mount(Landing, {});
  await settle();
  return m.root;
}

describe("the landing page's chrome", () => {
  it("frames the quickstart page under the shell's header, with no threads", async () => {
    const root = await landing(false);
    expect(root.querySelector("header.gbar h1")?.textContent).toBe("Clax");
    expect(root.querySelector("iframe.lframe")?.getAttribute("src")).toBe("/start/index.html");
    expect(root.querySelector(".threads, .drawer, button.comment")).toBeNull();
  });

  it("offers sign-in to a signed-out visitor", async () => {
    const root = await landing(false);
    expect(root.querySelector("header button.primary")?.textContent).toBe("Sign in");
  });

  it("offers a signed-in person their gallery", async () => {
    const root = await landing(true);
    expect(root.querySelector("header button.primary")?.textContent).toBe("Your gallery");
  });
});
