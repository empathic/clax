// assets.d.ts in the shell, on the phase 1 asset store. The shell enforces the
// contract's closed type set, its per-type size limits, and its text and markup
// checks before anything is sent; the daemon's own store (any image/, video/,
// or font/ type, 20 MiB) is the outer bound. Only the owner shell holds the
// token, so only it is offered the namespace (availability.ts).
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

type ApiAsset = { id: string; content_type: string; size: number; created_at: string };

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

/** Asset IDs as the daemon issues them (ULIDs). */
const ULID = /^[0-7][0-9A-HJKMNP-TV-Z]{25}$/;
/** What the page side accepts as an asset reference once a URL is stripped. */
const REF = /^[0-9A-Za-z]{1,64}$/;

const SNIFF_BYTES = 4096;

/** The bytes of `blob` (its first `end` when given), read with a FileReader. */
function bytesOf(blob: Blob, end?: number): Promise<Uint8Array> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(new Uint8Array(r.result as ArrayBuffer));
    r.onerror = () => reject(new CapError("invalid_request", "the file could not be read"));
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

/** Checks the body against the contract's rules for `type`; throws the rejection. */
async function checkBody(blob: Blob, type: string): Promise<void> {
  if (BINARY.includes(type)) {
    if (startsWithMarkup(await bytesOf(blob, SNIFF_BYTES))) throw new CapError("unsupported_type", `a ${type} body starts with markup; markup uploads only as image/svg+xml`);
    return;
  }
  if (type === "image/svg+xml") {
    if (!opensSvg(await bytesOf(blob, SNIFF_BYTES))) throw new CapError("unsupported_type", "the body is not an SVG document");
    return;
  }
  const bytes = await bytesOf(blob);
  try {
    new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    throw new CapError("invalid_request", `a ${type} file must be UTF-8; re-encode it before uploading`);
  }
  const nul = bytes.includes(0);
  if (CODE.includes(type)) {
    if (nul || startsWithMarkup(bytes)) throw new CapError("unsupported_type", `a ${type} file must read as text and not start with markup`);
  } else if (nul) {
    throw new CapError("invalid_request", `a ${type} file must be UTF-8 (this one reads as UTF-16 or binary); re-encode it before uploading`);
  }
}

export const assetsHandler: HandlerFactory = env => {
  const base = `/api/artifacts/${encodeURIComponent(env.aid)}/assets`;
  const auth = (): Record<string, string> => ({ authorization: `Bearer ${env.token ?? ""}` });
  const inflight = new Set<AbortController>();
  let disposed = false;
  const closed = () => new CapError("unavailable", "this view has closed");

  async function failure(res: Response): Promise<CapError> {
    const err = ((await res.json().catch(() => ({}))) as { error?: { code?: string; message?: string } }).error ?? {};
    const message = err.message ?? `HTTP ${res.status}`;
    if (res.status === 413 || err.code === "asset_too_large" || err.code === "body_too_large") return new CapError("too_large", message);
    if (err.code === "unsupported_type") return new CapError("unsupported_type", message);
    if (res.status === 404) return new CapError("quota_or_state", "the artifact cannot take assets now");
    if (res.status === 401) return new CapError("upstream_auth", "the daemon did not accept this view's token; reload the page");
    if (res.status === 400) return new CapError("invalid_request", message);
    return new CapError("upstream_error", message);
  }

  /** `fetch`, abortable by dispose; nothing it returns is used after dispose. */
  async function send<T>(url: string, init: RequestInit, read: (res: Response) => Promise<T>): Promise<T> {
    if (disposed) throw closed();
    const ac = new AbortController();
    inflight.add(ac);
    try {
      let res: Response;
      try {
        res = await fetch(url, { ...init, signal: ac.signal });
      } catch {
        if (disposed) throw closed();
        throw new CapError("store_unavailable", "the Artifax daemon could not be reached");
      }
      if (disposed) throw closed();
      const out = await read(res);
      if (disposed) throw closed();
      return out;
    } finally {
      inflight.delete(ac);
    }
  }

  async function upload(arg: unknown) {
    if (!arg || typeof arg !== "object") throw new CapError("invalid_request", "upload takes a non-empty Blob or File");
    const { blob, type } = arg as { blob?: unknown; type?: unknown };
    if (!(blob instanceof Blob) || blob.size === 0) throw new CapError("invalid_request", "upload takes a non-empty Blob or File");
    if (type !== undefined && typeof type !== "string") throw new CapError("invalid_request", "options.type is a string");
    const ct = type ?? blob.type;
    if (!ct) throw new CapError("invalid_request", "the file has no content type: pass options.type");
    if (!ACCEPTED_TYPES.includes(ct)) throw new CapError("unsupported_type", `'${ct}' is not an accepted type; exact media types only: ${ACCEPTED_TYPES.join(", ")}`);
    const limit = limitFor(ct);
    if (blob.size > limit) throw new CapError("too_large", `a ${ct} asset is at most ${limit} bytes`);
    await checkBody(blob, ct);
    const form = new FormData();
    form.set("file", new Blob([blob], { type: ct }), "upload");
    return send(base, { method: "POST", headers: auth(), body: form }, async res => {
      if (res.status !== 201) throw await failure(res);
      const { asset, url } = (await res.json()) as { asset: ApiAsset; url: string };
      return { id: asset.id, url, sizeBytes: asset.size, contentType: asset.content_type };
    });
  }

  return {
    async call(method, args) {
      if (disposed) throw closed();
      switch (method) {
        case "upload":
          return upload(args[0]);
        case "list":
          return send(base, { method: "GET" }, async res => {
            if (!res.ok) throw await failure(res);
            const { assets } = (await res.json()) as { assets: ApiAsset[] };
            return {
              assets: assets.map(a => ({ id: a.id, url: `/_blob/${a.id}`, contentType: a.content_type, sizeBytes: a.size, createdAt: a.created_at })),
              // The asset store has no per-artifact quota; the maxima say so.
              usage: { files: assets.length, bytes: assets.reduce((n, a) => n + a.size, 0), maxFiles: Number.MAX_SAFE_INTEGER, maxBytes: Number.MAX_SAFE_INTEGER },
            };
          });
        case "delete": {
          const id = args[0];
          if (typeof id !== "string" || !REF.test(id)) throw new CapError("invalid_request", "delete takes an asset ID or its URL");
          // Nothing is stored under an ID the store never issues.
          if (!ULID.test(id)) return { deleted: false };
          return send(`${base}/${encodeURIComponent(id)}`, { method: "DELETE", headers: auth() }, async res => {
            if (res.status === 204) return { deleted: true };
            if (res.status === 404) return { deleted: false };
            throw await failure(res);
          });
        }
        default:
          throw new CapError("capability_removed", `assets.${String(method)} is not part of this runtime`);
      }
    },
    dispose() {
      disposed = true;
      for (const ac of inflight) ac.abort();
      inflight.clear();
    },
  };
};
