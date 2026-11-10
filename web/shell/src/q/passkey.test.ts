import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { dispatchTrusted } from "../../../bridge/test/trusted";
import { settle, stubFetch } from "../people/fixtures";
import { flush, mount } from "../test/svelte";
import ConfirmationCard from "./ConfirmationCard.svelte";
import ConfirmToasts from "./ConfirmToasts.svelte";
import { type ConfirmationView, confirmations } from "./confirm.svelte";
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
/** A passkey registration's confirmation, held by the daemon. */
const registration = (id: string): ConfirmationView => ({
  ...card(), id, words: "add a passkey that confirms these decisions from now on", created_at: "2026-10-30T10:00:30.000Z",
  confirm_url: `http://localhost:7480/inbox?confirm=${id}`, decision: { kind: "register_passkey" },
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
    expect(m.root.querySelector(".coffer")!.textContent).toContain("Require a passkey to confirm");
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
    expect(m.root.querySelector(".coffer")!.textContent).toBe("Passkey added: Confirm now asks for your passkey.");
  });

  it("a registration held for confirmation rides on the card that offered it, and the daemon adds it once approved there", async () => {
    const REG = "cf_fedcba9876543210fedcba9876543210";
    let added = false;
    let open: ConfirmationView[] = [card()];
    const calls = stubFetch({
      "GET /api/passkeys": () => added ? settings({ require_passkey: true, required: true, passkeys: [{ credential_id: "AQ", alg: -7, created_at: "2026-10-30T10:00:00.000Z" }] }) : settings(),
      "GET /api/confirmations": () => ({ confirmations: open }),
      "POST /api/passkeys/challenge": challenge,
      // Held while an agent is live: the daemon keeps it with its confirmation.
      "POST /api/passkeys": () => { open = [card(), registration(REG)]; return [202, { status: "confirmation_pending", confirmation_id: REG, confirm_url: `http://localhost:7480/inbox?confirm=${REG}` }]; },
      [`POST /api/confirmations/${REG}`]: () => { added = true; open = [card()]; return { confirmation: { ...registration(REG), status: "approved" } }; },
    });
    await confirmations().refresh();
    await passkeySettings(true);
    const offering = mount(ConfirmationCard, { c: card(), now });
    const toasts = mount(ConfirmToasts, {});
    await settle();
    click(byText(offering.root, "Set up"));
    await settle();
    // One card: the registration's line stands in for a card of its own.
    expect(offering.root.querySelector(".pk-held")!.textContent).toBe("Agents are running, so adding this passkey needs your confirmation too.");
    expect([...offering.root.querySelectorAll(".coffer button")].map(b => b.textContent!.trim())).toEqual(["Confirm adding it", "Cancel"]);
    expect([...toasts.root.querySelectorAll<HTMLElement>(".confirm-toasts > .cfcard")].map(e => e.dataset.confirmation)).toEqual([ID]);
    click(byText(offering.root, "Confirm adding it"));
    await settle();
    // Approving it is all it takes: nothing is sent again.
    expect(calls.filter(c => c.method !== "GET").map(c => [c.method, c.url, c.body])).toEqual([
      ["POST", "/api/passkeys/challenge", { purpose: "register" }],
      ["POST", "/api/passkeys", { credential_id: "AQ", public_key: "Bg", alg: -7, authenticator_data: "Bw", client_data_json: "BQ" }],
      ["POST", `/api/confirmations/${REG}`, { approve: true }],
    ]);
    expect(offering.root.querySelector(".coffer")!.textContent).toBe("Passkey added: Confirm now asks for your passkey.");
  });

  it("a registration still waiting after a reload rides on another card, or shows alone, and approving its card adds it", async () => {
    const REG = "cf_1111222233334444555566667777aaaa";
    let open: ConfirmationView[] = [card(), registration(REG)];
    const calls = stubFetch({
      "GET /api/passkeys": settings(),
      "GET /api/confirmations": () => ({ confirmations: open }),
      [`POST /api/confirmations/${REG}`]: { confirmation: { ...registration(REG), status: "approved" } },
    });
    await passkeySettings(true);
    await confirmations().refresh();
    const toasts = mount(ConfirmToasts, {});
    await settle();
    const shown = () => [...toasts.root.querySelectorAll<HTMLElement>(".confirm-toasts > .cfcard")];
    expect(shown().map(e => e.dataset.confirmation)).toEqual([ID]);
    expect(byText(shown()[0], "Confirm adding it")).toBeDefined();
    // The other one answered, the registration is a card of its own, offering nothing.
    open = [registration(REG)];
    await confirmations().refresh();
    await settle();
    expect(shown().map(e => e.dataset.confirmation)).toEqual([REG]);
    expect(shown()[0].querySelector(".coffer")).toBeNull();
    click(byText(shown()[0], "Confirm"));
    await settle();
    expect(calls.filter(c => c.method === "POST")).toEqual([{ url: `/api/confirmations/${REG}`, method: "POST", body: { approve: true } }]);
  });

  it("Cancel declines a held registration's confirmation and offers setup again", async () => {
    const REG = "cf_99998888777766665555444433332222";
    const calls = stubFetch({
      "GET /api/passkeys": settings(),
      "GET /api/confirmations": { confirmations: [card(), registration(REG)] },
      [`POST /api/confirmations/${REG}`]: { confirmation: { ...registration(REG), status: "declined" } },
    });
    await passkeySettings(true);
    await confirmations().refresh();
    const m = mount(ConfirmationCard, { c: card(), now });
    await settle();
    click(byText(m.root, "Cancel"));
    await settle();
    expect(calls.filter(c => c.method === "POST")).toEqual([{ url: `/api/confirmations/${REG}`, method: "POST", body: { approve: false } }]);
    expect(m.root.querySelector(".pk-held")).toBeNull();
    expect(byText(m.root, "Set up")).toBeDefined();
    expect(m.root.querySelector(".pk-setup .err")).toBeNull();
  });

  it("hears a held registration's confirmation answered elsewhere: approved, it is set up; declined, it says so", async () => {
    for (const [REG, status] of [["cf_aaaabbbbccccddddeeeeffff00001111", "approved"], ["cf_aaaabbbbccccddddeeeeffff00002222", "declined"]] as const) {
      stubFetch({ "GET /api/passkeys": settings(), "GET /api/confirmations": { confirmations: [card(), registration(REG)] } });
      await passkeySettings(true);
      await confirmations().refresh();
      const m = mount(ConfirmationCard, { c: card(), now });
      await settle();
      expect(m.root.querySelector(".pk-held")).not.toBeNull();
      confirmations().take({ ...registration(REG), status });
      await settle();
      expect(m.root.querySelector(".pk-held")).toBeNull();
      if (status === "approved") expect(m.root.querySelector(".coffer")!.textContent).toBe("Passkey added: Confirm now asks for your passkey.");
      else expect(m.root.querySelector(".pk-setup .err")!.textContent).toBe("The passkey was not added: its confirmation was declined.");
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
    // It says a passkey is asked for; Settings lists them.
    expect(m.root.querySelector(".ckey")!.textContent).toBe("Confirm asks for your passkey.");
    expect(m.root.querySelector(".pk-list")).toBeNull();
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
    expect(m.root.querySelector(".pk-list")!.textContent).toContain("from the Clax app; no agents running then");
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
