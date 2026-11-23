import { afterEach, describe, expect, it, vi } from "vitest";
import { mount } from "../test/svelte";
import { settle, stubFetch } from "../people/fixtures";
import Landing from "./Landing.svelte";

afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); document.body.replaceChildren(); });

const me = (signed_in: boolean) => ({ origin: "https://clax.example", setup_open: false, signed_in, person: null });

async function landing(signedIn: boolean, given: boolean | null = null) {
  const calls = stubFetch({ "GET /api/hosted/me": me(signedIn) });
  const m = mount(Landing, { signedIn: given });
  await settle();
  return { root: m.root, calls };
}

describe("the landing page's header", () => {
  it("is the shell's header, with no threads; the quickstart's stage is the entry's frame's", async () => {
    const { root } = await landing(false);
    expect(root.querySelector("header.gbar h1")?.textContent).toBe("Clax");
    expect(root.querySelector("header.gbar .sub")).toBeNull();
    expect(root.querySelector("iframe")).toBeNull();
    expect(root.querySelector(".threads, .drawer, button.comment")).toBeNull();
  });

  it("offers sign-in to a signed-out visitor", async () => {
    const { root } = await landing(false);
    expect(root.querySelector("header a.primary")?.textContent).toBe("Sign in");
    expect(root.querySelector("header a.primary")?.getAttribute("href")).toBe("/signin");
  });

  it("offers a signed-in person their gallery", async () => {
    const { root } = await landing(true);
    expect(root.querySelector("header a.primary")?.textContent).toBe("Your gallery");
    expect(root.querySelector("header a.primary")?.getAttribute("href")).toBe("/");
  });

  it("takes the block's word for it, asking nothing", async () => {
    const { root, calls } = await landing(false, true);
    expect(root.querySelector("header a.primary")?.textContent).toBe("Your gallery");
    expect(calls).toEqual([]);
  });
});
