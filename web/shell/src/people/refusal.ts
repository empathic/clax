// A refusal as People, the inbox's cards and the invite dialog show it: a
// sentence of their own for the refusals a person can act on, else the
// daemon's message as a sentence. The daemon's messages are for agents and
// logs: lower case, no full stop.

const KNOWN: Record<string, string> = {
  name_taken: "That name is taken, or reads too like a name already here: choose another.",
  invalid_name: "That name cannot be used: choose another, without control or direction characters.",
};

/** What to show for error `e` (a `Refused`, or anything thrown). */
export function refusal(e: unknown): string {
  const code = typeof e === "object" && e && "code" in e ? (e as { code: unknown }).code : null;
  const text = e instanceof Error ? e.message : String(e);
  if (code === "wrong_code") return text.includes("denied") ? "That is not the code the machine shows, a third time: the request is denied." : "That is not the code the machine shows. Check it on that machine and type it again.";
  if (typeof code === "string" && KNOWN[code]) return KNOWN[code];
  const s = text.trim();
  if (!s) return "Something went wrong. Try again.";
  // A message that starts with a name or an ID ("p1 is not a person") keeps its case.
  const head = /^[a-z][a-z']*(\s|$)/.test(s) ? `${s[0].toUpperCase()}${s.slice(1)}` : s;
  return `${head}${/[.!?]$/.test(s) ? "" : "."}`;
}

/** Why People is not shown: `was` when this browser's People was open as a
 * connected person's (the owner has since disconnected them). */
export function peopleDenied(e: unknown, was: boolean): string {
  const code = typeof e === "object" && e && "code" in e ? (e as { code: unknown }).code : null;
  if (code !== "forbidden") return refusal(e);
  return was
    ? "You are no longer connected: the owner disconnected you. Ask them for a new invite to connect again."
    : "People is for the owner of this Clax and the people they connect. To connect, ask the owner for an invite.";
}
