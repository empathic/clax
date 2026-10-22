// A person's color and initials avatar (user.d.ts `color`, `avatarUrl`), the
// same in the bridge's `user` capability and the shell's lookups.

const PALETTE = ["#2b5fd9", "#c2410c", "#15803d", "#7c3aed", "#be185d", "#0e7490", "#a16207", "#4d7c0f"];
const NEUTRAL = "#6b7280";

/** A stable color per ID, readable under white initials in both themes; neutral without an ID. */
export function colorFor(id: string | null): string {
  if (!id) return NEUTRAL;
  let h = 0;
  for (const c of id) h = (Math.imul(h, 31) + c.charCodeAt(0)) >>> 0;
  return PALETTE[h % PALETTE.length];
}

/** An initials avatar on `color` as a data: URL (`?` when there is no name). */
export function avatarFor(name: string, color: string): string {
  const letters = name.trim().split(/\s+/).filter(Boolean).slice(0, 2).map(w => [...w][0].toUpperCase()).join("") || "?";
  const text = letters.replace(/[&<>"']/g, c => `&#${c.charCodeAt(0)};`);
  const fill = /^#[0-9a-f]{6}$/i.test(color) ? color : NEUTRAL;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64" viewBox="0 0 64 64"><circle cx="32" cy="32" r="32" fill="${fill}"/><text x="32" y="41" font-family="system-ui,sans-serif" font-size="26" font-weight="600" text-anchor="middle" fill="#ffffff">${text}</text></svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}
