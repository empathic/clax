// The reel's people, with the live colours a vignette casts them in. A
// person's arrow and name tag take a colour the daemon picks when they join
// an artifact's live layers (pick_color, crates/clax-core/src/live_layers.rs):
// a hash of their public ID, unless someone already there holds it. The
// daemon mints public IDs at random, so a vignette that wants the same
// colours each run mints invites until the person's ID hashes to the colour
// it wants (revoking the rest), and has its people join the page in an
// order where nobody holds another's colour.
import type { Browser, Page } from "@playwright/test";

/** How many live colours the daemon has (LIVE_COLORS). */
export const LIVE_COLORS = 8;

/** The shell's live colours by number (web/shell/src/view/live-layers.ts,
 * which people.test.ts checks this against). */
export const LIVE_PALETTE: readonly string[] = ["#2f62d0", "#8a3fd1", "#0a757b", "#be2f7a", "#975a00", "#6b6500", "#33708f", "#a3445a"];

/** How many invites `memberIn` mints, at most, to find a person a colour. */
export const MAX_MINTS = 128;

/** 32-bit FNV-1a of `s`'s UTF-8 bytes, as the daemon's `fnv1a`. */
export function fnv1a(s: string): number {
  let h = 0x811c9dc5;
  for (const b of new TextEncoder().encode(s)) h = Math.imul(h ^ b, 0x01000193) >>> 0;
  return h;
}

/** The live colour a person with `publicId` takes when it is free. */
export const preferredColor = (publicId: string) => fnv1a(publicId) % LIVE_COLORS;

type Minted = { id: string; url: string; person: string };

/**
 * A person `name` the owner (`owner`, a page on `origin` signed in as the
 * owner) invites with `rights`, whose preferred live colour is `color`,
 * joined in a browser of their own (a new context) at the gallery. Invites
 * whose person hashes to another colour are revoked, which frees the name.
 */
export async function memberIn(browser: Browser, owner: Page, origin: string, name: string, rights: unknown[], color: number): Promise<Page> {
  let url: string | null = null;
  for (let i = 0; i < MAX_MINTS && !url; i++) {
    const made = await owner.evaluate(async ([n, r]) => {
      const res = await fetch("/api/people/invites", { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify({ name: n, rights: r }) });
      if (!res.ok) throw new Error(`invite: ${res.status} ${await res.text()}`);
      const { invite } = await res.json();
      return { id: invite.id, url: invite.url, person: invite.person.public_id } as Minted;
    }, [name, rights] as const);
    if (preferredColor(made.person) === color) {
      url = made.url;
      break;
    }
    await owner.evaluate(async id => {
      const res = await fetch(`/api/people/invites/${encodeURIComponent(id)}`, { method: "DELETE" });
      if (!res.ok) throw new Error(`revoke: ${res.status} ${await res.text()}`);
    }, made.id);
  }
  if (!url) throw new Error(`no invite for ${name} hashed to live colour ${color} in ${MAX_MINTS} tries`);
  const page = await (await browser.newContext()).newPage();
  await page.goto(url);
  await page.getByRole("button", { name: `Join as ${name}` }).click();
  await page.waitForURL(`${origin}/`);
  return page;
}

/**
 * The live colour `page`'s next socket on an artifact's live layers is
 * welcomed with (the daemon's `{"t": "welcome", "color"}`): listen before
 * the page opens the artifact.
 */
export function welcomedColor(page: Page): Promise<number> {
  return new Promise(resolve => {
    page.on("websocket", ws => {
      ws.on("framereceived", ({ payload }) => {
        try {
          const m = JSON.parse(String(payload)) as { t?: string; color?: number };
          if (m.t === "welcome" && typeof m.color === "number") resolve(m.color);
        } catch {
          // Not JSON: not the welcome.
        }
      });
    });
  });
}
