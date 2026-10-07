// How the panel and the composer tell the person to get screenshots in a
// tab Clax is on (spec 2026-10-05 L8): the keyboard command, which grants
// activeTab (the toolbar icon there would turn Clax off), named by the
// shortcut Chrome has assigned it, or the page's context menu when it has
// none. Every such wording is here.

/** The command's assigned shortcut ("⌥⇧C" on macOS), or null when none is. */
export type Shortcut = string | null;

/** Reads the `comment` command's shortcut from Chrome; null when unassigned or unreadable. */
export async function readShortcut(commands: Pick<typeof chrome.commands, "getAll"> | undefined = globalThis.chrome?.commands): Promise<Shortcut> {
  try {
    const all = await commands?.getAll();
    return all?.find(c => c.name === "comment")?.shortcut || null;
  } catch {
    return null;
  }
}

const how = (keys: Shortcut) => (keys ? `press ${keys} on the page` : "right-click the page and choose Comment with Clax");

/** The side panel's words when Comment needs a grant the tab lacks. */
export const panelHint = (keys: Shortcut) => {
  const h = how(keys);
  return `${h[0].toUpperCase()}${h.slice(1)} to comment with a screenshot.`;
};

/** The composer's words, after "No screenshot: ", for a pick taken without the grant. */
export const clipHint = (keys: Shortcut) => `${how(keys)} before your next pick to include one`;
