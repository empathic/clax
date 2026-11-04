import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import ConfirmationCard from "./ConfirmationCard.svelte";
import ConfirmToasts from "./ConfirmToasts.svelte";
import { ConfirmationFeed, type ConfirmationView, confirmations } from "./confirm.svelte";
import { type PasskeySettings as Settings, b64u, fromB64u, passkeySettings } from "./passkey";
import PasskeySettings from "./PasskeySettings.svelte";

const click = (el: Element) => flush(() => { dispatchTrusted(el, new MouseEvent("click", { bubbles: true, cancelable: true, detail: 1 })); });
const ID = "cf_0123456789abcdef0123456789abcdef";
const now = new Date("2026-10-30T10:01:00.000Z");
const bytes = (...b: number[]) => new Uint8Array(b).buffer;
const byText = (root: Element, t: string) => [...root.querySelectorAll<HTMLButtonElement>("button")].find(b => b.textContent!.trim() === t)!;

const card = (): ConfirmationView => ({
  id: ID, status: "open", words: "push “Plan” to https://clax.dev", created_at: "2026-10-30T10:00:00.000Z",
  expires_at: "2026-10-30T10:10:00.000Z", confirm_url: `http://localhost:7480/inbox?confirm=${ID}`, agents: [],
});
const settings = (over: Partial<Settings> = {}): Settings => ({
  available: true, unavailable_reason: null, rp_id: "localhost", origin: "http://localhost:7480",
  require_passkey: false, required: false, offer_dismissed: false, passkeys: [], ...over,
});
const challenge = { challenge: b64u(bytes(1, 2, 3)), rp_id: "localhost", user_id: b64u(bytes(9)), timeout_ms: 600000, allow: ["AQ"] };

let created: unknown[] = [];
let got: unknown[] = [];
beforeEach(() => {
  created = []; got = [];
  vi.stubGlobal("isSecureContext", true);
  vi.stubGlobal("PublicKeyCredential", {});
  Object.defineProperty(navigator, "credentials", {
    configurable: true,
    value: {
      create: vi.fn(async (o: unknown) => {
        created.push(o);
        return { rawId: bytes(1), response: { clientDataJSON: bytes(5), getPublicKey: () => bytes(6), getPublicKeyAlgorithm: () => -7, getAuthenticatorData: () => bytes(7) } };
      }),
      get: vi.fn(async (o: unknown) => {
        got.push(o);
        return { rawId: bytes(1), response: { clientDataJSON: bytes(5), authenticatorData: bytes(7), signature: bytes(8) } };
      }),
    },
  });
});
afterEach(() => { vi.unstubAllGlobals(); document.body.replaceChildren(); });

describe("passkeys for confirmations", () => {
  it("round-trips base64url", () => {
    expect(b64u(fromB64u("AQID_-8"))).toBe("AQID_-8");
  });

  it("offers a passkey on first use, and Don't ask again is remembered", async () => {
    const calls = stubFetch({ "GET /api/passkeys": settings(), "PUT /api/passkeys/settings": settings({ offer_dismissed: true }) });
    await passkeySettings(true);
    const m = mount(ConfirmationCard, { c: card(), now });
    await settle();
    expect(m.root.querySelector(".coffer")!.textContent).toContain("Require Touch ID or a passkey to confirm");
    click(byText(m.root, "Don't ask again"));
    await settle();
    expect(calls.filter(c => c.method === "PUT")).toEqual([{ url: "/api/passkeys/settings", method: "PUT", body: { offer_dismissed: true } }]);
    expect(m.root.querySelector(".coffer")).toBeNull();
  });

  it("sets one up from the offer, user verified, and then requires it", async () => {
    const calls = stubFetch({
      "GET /api/passkeys": settings(),
      "POST /api/passkeys/challenge": challenge,
      "POST /api/passkeys": { passkey: { credential_id: "AQ" }, settings: settings({ passkeys: [{ credential_id: "AQ", alg: -7, created_at: "2026-10-30T10:00:00.000Z" }] }) },
      "PUT /api/passkeys/settings": settings({ require_passkey: true, required: true }),
    });
    await passkeySettings(true);
    const m = mount(ConfirmationCard, { c: card(), now });
    await settle();
    click(byText(m.root, "Set up"));
    await settle();
    const pk = (created[0] as { publicKey: PublicKeyCredentialCreationOptions }).publicKey;
    expect(pk.authenticatorSelection?.userVerification).toBe("required");
    expect(pk.rp.id).toBe("localhost");
    expect(calls.filter(c => c.method !== "GET").map(c => [c.method, c.url, c.body])).toEqual([
      ["POST", "/api/passkeys/challenge", { purpose: "register" }],
      ["POST", "/api/passkeys", { credential_id: "AQ", public_key: "Bg", alg: -7, authenticator_data: "Bw", client_data_json: "BQ" }],
      ["PUT", "/api/passkeys/settings", { required: true }],
    ]);
    expect(m.root.querySelector(".coffer")!.textContent).toBe("Passkey added: Confirm now asks for Touch ID or your passkey.");
  });

  it("keeps a registration held for a confirmation past the card that offered it, and finishes it once that confirmation is approved", async () => {
    const REG = "cf_fedcba9876543210fedcba9876543210";
    let posts = 0;
    const calls = stubFetch({
      "GET /api/passkeys": settings(),
      "POST /api/passkeys/challenge": challenge,
      // Held while an agent is live; presented with its confirmation, registered.
      "POST /api/passkeys": () => posts++ === 0
        ? [202, { status: "confirmation_pending", confirmation_id: REG, confirm_url: `http://localhost:7480/inbox?confirm=${REG}` }]
        : [201, { passkey: { credential_id: "AQ" }, settings: settings({ passkeys: [{ credential_id: "AQ", alg: -7, created_at: "2026-10-30T10:00:00.000Z" }] }) }],
      [`POST /api/confirmations/${ID}`]: { confirmation: { ...card(), status: "approved" } },
      [`POST /api/confirmations/${REG}`]: { confirmation: { ...card(), id: REG, status: "approved" } },
      "PUT /api/passkeys/settings": settings({ require_passkey: true, required: true }),
    });
    await passkeySettings(true);
    const offering = mount(ConfirmationCard, { c: card(), now });
    await settle();
    click(byText(offering.root, "Set up"));
    await settle();
    // The next step is on the line itself, which stands in for the registration's card.
    expect(offering.root.querySelector(".pk-held")!.textContent).toBe("Agents are running, so adding this passkey needs your confirmation too.");
    expect([...offering.root.querySelectorAll(".coffer button")].map(b => b.textContent!.trim())).toEqual(["Confirm adding it", "Cancel"]);
    // So the page lists every open confirmation but that one.
    const feed = new ConfirmationFeed({ list: async () => [card(), { ...card(), id: REG }] });
    await feed.refresh();
    const toasts = mount(ConfirmToasts, { feed });
    await settle();
    expect([...toasts.root.querySelectorAll<HTMLElement>(".confirm-toasts .cfcard")].map(e => e.dataset.confirmation)).toEqual([ID]);
    toasts.unmount();
    // Answering the card that offered it drops the card from the inbox.
    click(byText(offering.root, "Confirm"));
    await settle();
    offering.unmount();
    // The registration's own card offers nothing; another card shows the setup still waiting.
    const reg = mount(ConfirmationCard, { c: { ...card(), id: REG }, now });
    const other = mount(ConfirmationCard, { c: { ...card(), id: "cf_other" }, now });
    await settle();
    expect(reg.root.querySelector(".coffer")).toBeNull();
    expect(byText(other.root, "Set up")).toBeUndefined();
    // Confirming it there answers the registration's confirmation and finishes the setup.
    click(byText(other.root, "Confirm adding it"));
    await settle();
    expect(other.root.querySelector(".pk-held")).toBeNull();
    expect(calls.filter(c => c.method !== "GET").map(c => [c.method, c.url])).toEqual([
      ["POST", "/api/passkeys/challenge"],
      ["POST", "/api/passkeys"],
      ["POST", `/api/confirmations/${ID}`],
      ["POST", `/api/confirmations/${REG}`],
      ["POST", "/api/passkeys"],
      ["PUT", "/api/passkeys/settings"],
    ]);
    expect(other.root.querySelector(".coffer")!.textContent).toBe("Passkey added: Confirm now asks for Touch ID or your passkey.");
  });

  it("Cancel declines a held registration's confirmation and ends the setup", async () => {
    const REG = "cf_99998888777766665555444433332222";
    const calls = stubFetch({
      "GET /api/passkeys": settings(),
      "POST /api/passkeys/challenge": challenge,
      "POST /api/passkeys": [202, { status: "confirmation_pending", confirmation_id: REG, confirm_url: `http://localhost:7480/inbox?confirm=${REG}` }],
      [`POST /api/confirmations/${REG}`]: { confirmation: { ...card(), id: REG, status: "declined" } },
    });
    await passkeySettings(true);
    const m = mount(ConfirmationCard, { c: card(), now });
    await settle();
    click(byText(m.root, "Set up"));
    await settle();
    click(byText(m.root, "Cancel"));
    await settle();
    expect(calls.filter(c => c.method === "POST").at(-1)).toEqual({ url: `/api/confirmations/${REG}`, method: "POST", body: { approve: false } });
    expect(m.root.querySelector(".pk-held")).toBeNull();
    expect(byText(m.root, "Set up")).toBeDefined();
    expect(m.root.querySelector(".pk-setup .err")).toBeNull();
  });

  it("finishes a held registration when the page hears its confirmation approved elsewhere, and ends it when declined", async () => {
    const REG = "cf_aaaabbbbccccddddeeeeffff00001111";
    for (const [status, done] of [["approved", true], ["declined", false]] as const) {
      let posts = 0;
      const calls = stubFetch({
        "GET /api/passkeys": settings(),
        "POST /api/passkeys/challenge": challenge,
        "POST /api/passkeys": () => posts++ === 0
          ? [202, { status: "confirmation_pending", confirmation_id: REG, confirm_url: `http://localhost:7480/inbox?confirm=${REG}` }]
          : [201, { passkey: { credential_id: "AQ" }, settings: settings({ passkeys: [{ credential_id: "AQ", alg: -7, created_at: "2026-10-30T10:00:00.000Z" }] }) }],
        "PUT /api/passkeys/settings": settings({ require_passkey: true, required: true }),
      });
      await passkeySettings(true);
      const m = mount(ConfirmationCard, { c: card(), now });
      await settle();
      click(byText(m.root, "Set up"));
      await settle();
      // Answered in another tab: the page's feed hears it.
      confirmations().take({ ...card(), id: REG, status });
      await settle();
      expect(calls.filter(c => c.method !== "GET").map(c => c.method + " " + c.url)).toEqual(done
        ? ["POST /api/passkeys/challenge", "POST /api/passkeys", "POST /api/passkeys", "PUT /api/passkeys/settings"]
        : ["POST /api/passkeys/challenge", "POST /api/passkeys"]);
      expect(m.root.querySelector(".pk-held")).toBeNull();
      if (!done) expect(m.root.querySelector(".pk-setup .err")!.textContent).toBe("The passkey was not added: its confirmation was declined.");
      m.unmount();
    }
  });

  it("confirms with an assertion where passkeys are required", async () => {
    const calls = stubFetch({
      "GET /api/passkeys": settings({ require_passkey: true, required: true, passkeys: [{ credential_id: "AQ", alg: -7, created_at: "2026-10-30T10:00:00.000Z", added_by: { via: "a browser session", agents_live: ["ada (claude)"] } }] }),
      "POST /api/passkeys/challenge": challenge,
      [`POST /api/confirmations/${ID}`]: { confirmation: { ...card(), status: "approved" } },
    });
    await passkeySettings(true);
    const m = mount(ConfirmationCard, { c: card(), now });
    await settle();
    expect(m.root.querySelector(".coffer")).toBeNull();
    // The passkeys it asks for say how each was added.
    expect(m.root.querySelector(".ckeys")!.textContent).toContain("from a browser session; agents running then: ada (claude)");
    click(byText(m.root, "Confirm"));
    await settle();
    expect((got[0] as { publicKey: PublicKeyCredentialRequestOptions }).publicKey.userVerification).toBe("required");
    expect(calls.filter(c => c.method === "POST").map(c => [c.url, c.body])).toEqual([
      ["/api/passkeys/challenge", { purpose: "confirm", confirmation_id: ID }],
      [`/api/confirmations/${ID}`, { approve: true, assertion: { credential_id: "AQ", authenticator_data: "Bw", client_data_json: "BQ", signature: "CA" } }],
    ]);
    expect(m.root.querySelector(".outcome")!.textContent).toBe("Confirmed");
  });
});

describe("the Settings switch", () => {
  const on = settings({ require_passkey: true, required: true, passkeys: [{ credential_id: "AQ", alg: -7, created_at: "2026-10-30T09:00:00.000Z", added_by: { via: "the Clax app", agents_live: [] } }] });
  const sw = (root: Element) => root.querySelector<HTMLButtonElement>('[role="switch"]')!;

  it("turns off only with a passkey's assertion", async () => {
    const calls = stubFetch({
      "GET /api/passkeys": on,
      "POST /api/passkeys/challenge": challenge,
      "PUT /api/passkeys/settings": { ...on, require_passkey: false, required: false },
    });
    const m = mount(PasskeySettings, { now });
    await settle();
    expect(sw(m.root).getAttribute("aria-checked")).toBe("true");
    expect(m.root.querySelector(".pk-list")!.textContent).toContain("from the Clax app; agents running then: none");
    expect(m.root.textContent).toContain("clax passkeys reset");
    click(sw(m.root));
    await settle();
    expect(calls.filter(c => c.method !== "GET").map(c => [c.method, c.url, c.body])).toEqual([
      ["POST", "/api/passkeys/challenge", { purpose: "disable" }],
      ["PUT", "/api/passkeys/settings", { required: false, assertion: { credential_id: "AQ", authenticator_data: "Bw", client_data_json: "BQ", signature: "CA" } }],
    ]);
    expect(sw(m.root).getAttribute("aria-checked")).toBe("false");
  });

  it("says why where no passkey can be used, and stays off", async () => {
    stubFetch({ "GET /api/passkeys": settings({ available: false, unavailable_reason: "Passkeys need a secure address: open Clax at http://localhost to use one" }) });
    const m = mount(PasskeySettings, { now });
    await settle();
    expect(sw(m.root).getAttribute("aria-disabled")).toBe("true");
    expect(sw(m.root).getAttribute("aria-checked")).toBe("false");
    expect(m.root.querySelector("#pk-why")!.textContent).toContain("secure address");
    expect([...m.root.querySelectorAll("button")].some(b => b.textContent === "Add a passkey")).toBe(false);
  });
});
