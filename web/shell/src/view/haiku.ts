// Clax's haiku in the shell (spec §8, "Look"): the gallery footer picks one
// at random each visit; the working line picks by the record's key, so it
// holds still while the record lives. The list is haiku.json, fetched after first paint.
export function pickHaiku(list: string[], seed?: string): string {
  if (seed === undefined) return list[Math.floor(Math.random() * list.length)];
  let h = 0x811c9dc5;
  for (let i = 0; i < seed.length; i++) { h ^= seed.charCodeAt(i); h = Math.imul(h, 0x01000193) >>> 0; }
  return list[h % list.length];
}
