// artifact.publish(html) in the shell (artifact.d.ts): republish the page the
// call came from as a new version, compare-and-set on the version the frame
// shows. The html replaces the file that page was served from (`env.page()`,
// from its hello); every other file of the version is carried forward
// unchanged, `index.html` included (the daemon wants it on every publish, so a
// sub page's publish resends its stored bytes). The owner shell writes with
// its token; any other view is not a writer. Success and conflict both reload
// this view to the live version; other open views reload on the SSE `version`
// event, which carries `by_page`.
import { INDEX_FILE } from "../../../bridge/src/protocol";
import { type Version, getArtifact } from "../api";
import { CapError } from "./errors";
import type { HandlerFactory } from "./host";

/** Largest page a publish accepts, as the daemon's per-file cap. */
export const MAX_PAGE_BYTES = 16 * 1024 * 1024;
const DOCTYPE = /^[\s﻿]*<!doctype/i;
/** Delay before reloading, so the call result reaches the page first. */
export const RELOAD_DELAY_MS = 50;

type FileBody = { content: string; encoding: "utf8" | "base64"; content_type?: string };

function base64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}

/** The html argument, checked before anything is requested. */
function checkHtml(html: unknown): string {
  if (html !== null && typeof html === "object" && !Array.isArray(html)) {
    throw new CapError("capability_disabled", "the files form of publish is not available in Artifax; publish a complete HTML page");
  }
  if (typeof html !== "string" || !DOCTYPE.test(html)) throw new CapError("invalid_content", "publish takes a complete page that begins with <!doctype html>");
  if (new Blob([html]).size > MAX_PAGE_BYTES) throw new CapError("too_large", `a page is at most ${MAX_PAGE_BYTES} bytes`);
  return html;
}

export const artifactHandler: HandlerFactory = env => {
  let disposed = false;
  let reloadTimer: ReturnType<typeof setTimeout> | null = null;
  // Whether this handler set `env.ownPublish.active` (and so clears it on dispose).
  let claimed = false;
  const claim = (on: boolean) => {
    claimed = on;
    if (env.ownPublish) env.ownPublish.active = on;
  };
  const closed = () => new CapError("upstream_error", "this view has closed");
  const reloadSoon = () => {
    if (reloadTimer === null) reloadTimer = setTimeout(() => { reloadTimer = null; if (!disposed) env.reload(); }, RELOAD_DELAY_MS);
  };
  const base = `/api/artifacts/${encodeURIComponent(env.aid)}`;

  /** The files of the publish: the calling page's new html, plus the stored
   * index when the caller is a sub page. */
  async function filesFor(page: string, html: string, version: Version | undefined): Promise<Record<string, FileBody>> {
    if (page === INDEX_FILE) return { [INDEX_FILE]: { content: html, encoding: "utf8" } };
    const meta = version?.files;
    if (!meta || !Object.hasOwn(meta, page) || !Object.hasOwn(meta, INDEX_FILE)) {
      throw new CapError("upstream_error", `v${env.version} of this artifact could not be read`);
    }
    let res: Response;
    try {
      res = await fetch(`${base}/versions/${env.version}/files/${INDEX_FILE}`);
    } catch {
      throw new CapError("upstream_error", "the Artifax daemon could not be reached");
    }
    if (!res.ok) throw new CapError("upstream_error", `the index of v${env.version} could not be read (HTTP ${res.status})`);
    const index = base64(new Uint8Array(await res.arrayBuffer()));
    return {
      [INDEX_FILE]: { content: index, encoding: "base64", content_type: meta[INDEX_FILE].content_type },
      [page]: { content: html, encoding: "utf8", content_type: meta[page].content_type },
    };
  }

  return {
    async call(method, args) {
      if (disposed) throw closed();
      if (method === "edit" || method === "sync") throw new CapError("invalid_content", "this artifact is not a live doc");
      if (method !== "publish") throw new CapError("capability_removed", `artifact.${String(method)} is not part of this runtime`);
      const html = checkHtml(args[0]);
      if (!env.token) throw new CapError("not_writer", "this view can see the page but cannot publish it");
      const page = env.page ? env.page() : INDEX_FILE;
      if (page === null) throw new CapError("upstream_error", "the page in this view is not known yet");
      // Set before the lookup, so this view's own SSE `version` event cannot reload it first.
      claim(true);
      try {
        const fresh = await getArtifact(env.aid).catch(() => null);
        if (disposed) throw closed();
        const caps = fresh?.artifact.capabilities ?? {};
        if (fresh && !Object.hasOwn(caps, "artifact") && !Object.hasOwn(caps, "self")) {
          throw new CapError("not_declared", "the artifact no longer declares the artifact capability");
        }
        const files = await filesFor(page, html, fresh?.versions.find(v => v.n === env.version));
        if (disposed) throw closed();
        let res: Response;
        try {
          res = await fetch(`${base}/versions`, {
            method: "POST",
            headers: { "content-type": "application/json", authorization: `Bearer ${env.token}`, "x-artifax-via": "page" },
            body: JSON.stringify({ if_version: env.version, files }),
          });
        } catch {
          throw new CapError("upstream_error", "the Artifax daemon could not be reached");
        }
        if (disposed) throw closed();
        if (res.status === 201) {
          const v = (await res.json().catch(() => ({}))) as { version?: { n?: number } };
          reloadSoon();
          return { version: String(v.version?.n ?? "") };
        }
        const err = ((await res.json().catch(() => ({}))) as { error?: { code?: string; message?: string; current?: number } }).error ?? {};
        if (res.status === 409) {
          reloadSoon();
          throw new CapError("conflict", "a newer version was published first; this view is reloading to it", { live: String(err.current ?? "") });
        }
        if (res.status === 413 || err.code === "file_too_large" || err.code === "body_too_large") throw new CapError("too_large", err.message ?? "too large");
        if (res.status === 401) throw new CapError("not_writer", "this view can see the page but cannot publish it");
        if (res.status === 400) throw new CapError("invalid_content", err.message ?? "the daemon refused the page");
        throw new CapError("upstream_error", err.message ?? `HTTP ${res.status}`);
      } finally {
        // A scheduled reload keeps the claim: this view is about to leave.
        if (reloadTimer === null) claim(false);
      }
    },
    dispose() {
      disposed = true;
      if (reloadTimer !== null) clearTimeout(reloadTimer);
      reloadTimer = null;
      if (claimed) claim(false);
    },
  };
};
