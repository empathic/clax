// What a hosted daemon's sign-in pages and landing page need before their
// first render, from the daemon's bootstrap block
// (crates/clax-server/src/shell/signin.rs documents the contract): each
// field the body its route answers this browser's own `fetch`, and
// `status[f]` that route's status; a field with no status is null, and the
// page asks its route itself.
import type { DeviceRequest, HostedMe, PublicArtifact } from "./api";

/** Which of these pages a path is: the landing page (`/start/`), the
 * first person's setup, an invite, a one-use sign-in link, a device's
 * sign-in, or `/signin` itself (the browser's status). The entry's inline
 * `clax-page` script decides it the same way (a test holds them to each
 * other), to show the page's frame before any script loads. */
export type AuthKind = "landing" | "setup" | "join" | "signin" | "device" | "status";

export function authKind(path: string): AuthKind {
  if (path === "/start/") return "landing";
  if (path === "/setup") return "setup";
  if (path === "/device") return "device";
  if (path.startsWith("/join/")) return "join";
  if (path.startsWith("/signin/") && path.length > "/signin/".length) return "signin";
  return "status";
}

/** Each page's heading, as its frame and the page write it. */
export const AUTH_TITLE: Record<Exclude<AuthKind, "landing">, string> = { setup: "Set up Clax", join: "Join Clax", signin: "Sign in", device: "Connect a Clax", status: "Sign in" };

/** A field of the block and the route that answers it. */
export type SignInField = "hosted" | "public" | "invite" | "link" | "device";

export type SignInBoot = {
  v: 1;
  kind: "signin";
  status: Partial<Record<SignInField, number>>;
  /** `GET /api/hosted/me`. */
  hosted: HostedMe | null;
  /** `GET /api/hosted/public`, on `/signin`. */
  public: { artifacts: PublicArtifact[] } | null;
  /** `GET /api/hosted/invites/<code>`, on `/join/<code>`. */
  invite: { name: string | null; expires_at: string } | null;
  /** `GET /api/hosted/signin-links/<code>`, on `/signin/<code>`. */
  link: { public_id: string; name: string | null; owner: boolean; expires_at: string } | null;
  /** `GET /api/hosted/device/<code>`, on `/device?code=<code>` for a member. */
  device: DeviceRequest | null;
};

/** `f`'s status in the block, when its route answered it. */
export function statusOf(b: SignInBoot | null, f: SignInField): number | null {
  const s = b?.status[f];
  return typeof s === "number" ? s : null;
}

function navigationType(): string {
  try {
    return (performance.getEntriesByType("navigation")[0] as PerformanceNavigationTiming | undefined)?.type ?? "";
  } catch {
    return "";
  }
}

/** The sign-in pages' block in `doc`, or null when there is none, it does
 * not parse, or it is not a version 1 sign-in block. A page reached
 * through history reads none of it, as the gallery's does
 * (`view/gallery-boot.ts`): it may be another session's. */
export function readSignInBoot(doc: Document = document, navType: string = navigationType()): SignInBoot | null {
  if (navType === "back_forward") return null;
  const text = doc.getElementById("clax-boot")?.textContent;
  if (!text) return null;
  let b: SignInBoot;
  try {
    b = JSON.parse(text) as SignInBoot;
  } catch {
    return null;
  }
  return b && b.v === 1 && b.kind === "signin" && b.status && typeof b.status === "object" ? b : null;
}
