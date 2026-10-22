// Finding an app by its title: case- and accent-insensitive, matches where
// a word of the title starts with the query before those that contain it
// anywhere, each kept in the home screen's order.

/** `s` without case or accents. */
export const fold = (s) => String(s ?? '').normalize('NFD').replace(/\p{M}+/gu, '').toLowerCase();

const LETTER = /[\p{L}\p{N}]/u;

const atWordStart = (title, q) => {
  for (let i = title.indexOf(q); i >= 0; i = title.indexOf(q, i + 1)) {
    if (i === 0 || !LETTER.test(title[i - 1])) return true;
  }
  return false;
};

/** The entries ({app, group}, in the home screen's order: Starred, then
 * All apps with a group's apps at its place) whose app's title matches
 * `query`. */
export function search(query, entries) {
  const q = fold(query).trim();
  if (!q) return [];
  const starts = [];
  const inside = [];
  for (const entry of entries) {
    const title = fold(entry.app.title);
    if (atWordStart(title, q)) starts.push(entry);
    else if (title.includes(q)) inside.push(entry);
  }
  return [...starts, ...inside];
}
