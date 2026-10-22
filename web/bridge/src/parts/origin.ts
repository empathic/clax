// The capabilities of the page's own origin (`db`, `assets`, `user`, `room`):
// the site's client, what `use()` resolves of them, their members and the
// room's socket. Loaded on the page's first claude.use() of one of them,
// framed or top-level, never for the shell's capabilities (parts/caps.ts).
export { originUse } from "../caps/origin";
