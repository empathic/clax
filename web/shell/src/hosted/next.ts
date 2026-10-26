// Where a sign-in page returns to (`?next=`, which `/site-signin` sends a
// signed-out browser with: crates/clax-server/src/routes/site_signin.rs).
// The rules are the server's (crates/clax-http/src/site_access.rs
// `safe_next`, and crates/clax-server/src/github.rs `return_path` for the
// GitHub flow, which carries it): a path on this origin of at most 2048
// characters that starts with one `/`, holds no backslash, control
// character or anything past ASCII, and resolves on this origin. Anything
// else, absent included, is null: the page goes where it went before.

const MAX_NEXT = 2048;

/** `raw` as a path on `origin` a sign-in may return to, or null. */
export function safeNext(raw: string | null | undefined, origin: string): string | null {
  if (!raw || raw.length > MAX_NEXT) return null;
  if (!raw.startsWith("/") || raw.startsWith("//") || raw.includes("\\")) return null;
  // Control characters (which a browser strips from a URL) and anything a
  // `Location` would not hold as it is.
  if (!/^[\x20-\x7e]*$/.test(raw)) return null;
  try {
    if (new URL(raw, origin).origin !== new URL(origin).origin) return null;
  } catch {
    return null;
  }
  return raw;
}

/** The current page's `?next=` as [`safeNext`] takes it. */
export function nextOfPage(): string | null {
  return safeNext(new URLSearchParams(location.search).get("next"), location.origin);
}

/** `url` as a page on this machine a device sign-in may return to, or null: the rule of crates/clax-http/src/loopback.rs `loopback_return` (scheme `http`; host `localhost`, `127.0.0.1` or `[::1]`; no userinfo or fragment; at most 2048 bytes). */
export function loopbackReturn(url: string | null | undefined): string | null {
  if (!url || url.length > MAX_NEXT) return null;
  try {
    const u = new URL(url);
    const local = u.hostname === "localhost" || u.hostname === "127.0.0.1" || u.hostname === "[::1]";
    if (u.protocol !== "http:" || !local || u.username || u.password || u.hash || url.includes("#")) return null;
    return u.href.length <= MAX_NEXT ? u.href : null;
  } catch {
    return null;
  }
}
