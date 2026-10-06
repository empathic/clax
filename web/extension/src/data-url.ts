// `data:` URLs without `fetch`: the extension's pages and its worker may
// fetch only the daemon (their policy's `connect-src`), so a clip carried
// as a `data:` URL is encoded and decoded here.

/** The bytes of a base64 `data:` URL, typed as it says. */
export function dataUrlBlob(url: string): Blob {
  const comma = url.indexOf(",");
  const type = url.slice(5, comma).replace(/;base64$/, "");
  const bin = atob(url.slice(comma + 1));
  const bytes = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
  return new Blob([bytes], { type });
}

/** `bytes` as a base64 `data:` URL of `type`. */
export function bytesDataUrl(bytes: Uint8Array, type: string): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return `data:${type};base64,${btoa(s)}`;
}
