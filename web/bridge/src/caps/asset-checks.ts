// assets.d.ts's rules for an upload, checked in the page before anything is
// sent: the contract's closed type set, its per-type size limits, and its
// text and markup checks. The site's own store (any image/, video/, or font/
// type, 20 MiB) is the outer bound, and decides.
import { CapabilityError } from "../rpc";

/** Largest asset of most types, in bytes (the daemon's `MAX_ASSET_BYTES`). */
export const MAX_BYTES = 20 * 1024 * 1024;
/** Largest SVG asset, in bytes. */
export const SVG_MAX_BYTES = 2 * 1024 * 1024;
/** Largest stylesheet or script asset, in bytes. */
export const CODE_MAX_BYTES = 16 * 1024 * 1024;

const BINARY = ["image/png", "image/jpeg", "image/gif", "image/webp", "video/mp4", "video/webm", "application/pdf", "font/woff2", "font/woff", "font/ttf", "font/otf"];
const DATA = ["text/csv", "text/markdown", "application/json", "text/plain"];
const CODE = ["text/css", "text/javascript"];
/** The contract's accepted content types, exact spellings only. */
export const ACCEPTED_TYPES: readonly string[] = [...BINARY, "image/svg+xml", ...DATA, ...CODE];

/** The size limit of an accepted `type`, in bytes. */
export function limitFor(type: string): number {
  if (type === "image/svg+xml") return SVG_MAX_BYTES;
  if (CODE.includes(type)) return CODE_MAX_BYTES;
  return MAX_BYTES;
}

const SNIFF_BYTES = 4096;
const fail = (code: string, message: string) => new CapabilityError(code, message);

/** The bytes of `blob` (its first `end` when given), read with a FileReader. */
function bytesOf(blob: Blob, end?: number): Promise<Uint8Array> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(new Uint8Array(r.result as ArrayBuffer));
    r.onerror = () => reject(fail("invalid_request", "the file could not be read"));
    r.readAsArrayBuffer(end === undefined ? blob : blob.slice(0, end));
  });
}

/** Whether the first non-blank byte (after a UTF-8 byte-order mark) is `<`. */
function startsWithMarkup(b: Uint8Array): boolean {
  let i = b[0] === 0xef && b[1] === 0xbb && b[2] === 0xbf ? 3 : 0;
  while (i < b.length && (b[i] === 0x20 || b[i] === 0x09 || b[i] === 0x0a || b[i] === 0x0d || b[i] === 0x0c)) i++;
  return b[i] === 0x3c;
}

/** Whether `head` opens an SVG document: `<svg` after any prolog, comments, and doctype. */
function opensSvg(head: Uint8Array): boolean {
  let s = new TextDecoder().decode(head).replace(/^﻿/, "");
  for (;;) {
    const t = s.replace(/^\s+/, "");
    const next = t.replace(/^<\?[\s\S]*?\?>/, "").replace(/^<!--[\s\S]*?-->/, "").replace(/^<!DOCTYPE[^>[]*(\[[\s\S]*?\])?\s*>/i, "");
    if (next === t) return /^<svg[\s/>]/.test(t);
    s = next;
  }
}

/** The content type of an upload of `blob` (`type`, else the blob's own),
 * once it meets the contract's rules; throws the rejection. */
export async function checkUpload(blob: Blob, type: string | undefined): Promise<string> {
  const ct = type ?? blob.type;
  if (!ct) throw fail("invalid_request", "the file has no content type: pass options.type");
  if (!ACCEPTED_TYPES.includes(ct)) throw fail("unsupported_type", `'${ct}' is not an accepted type; exact media types only: ${ACCEPTED_TYPES.join(", ")}`);
  const limit = limitFor(ct);
  if (blob.size > limit) throw fail("too_large", `a ${ct} asset is at most ${limit} bytes`);
  if (BINARY.includes(ct)) {
    if (startsWithMarkup(await bytesOf(blob, SNIFF_BYTES))) throw fail("unsupported_type", `a ${ct} body starts with markup; markup uploads only as image/svg+xml`);
    return ct;
  }
  if (ct === "image/svg+xml") {
    if (!opensSvg(await bytesOf(blob, SNIFF_BYTES))) throw fail("unsupported_type", "the body is not an SVG document");
    return ct;
  }
  const bytes = await bytesOf(blob);
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    throw fail("invalid_request", `a ${ct} file must be UTF-8; re-encode it before uploading`);
  }
  const nul = bytes.includes(0);
  if (CODE.includes(ct)) {
    if (nul || startsWithMarkup(bytes)) throw fail("unsupported_type", `a ${ct} file must read as text and not start with markup`);
  } else if (nul) {
    throw fail("invalid_request", `a ${ct} file must be UTF-8 (this one reads as UTF-16 or binary); re-encode it before uploading`);
  }
  return ct;
}
