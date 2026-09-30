// downloads.save in the shell (downloads.d.ts): check the name against the
// contract's allowlist, show the viewer the final name and size, and hand an
// accepted file to the browser's download. One undecided prompt at a time
// (first wins), and at most [`PROMPTS_PER_WINDOW`] prompts per
// [`PROMPT_WINDOW_MS`] per artifact in a tab, saved or declined, so a page
// cannot loop the dialog. Ordinary saves have no size limit; export answers (`request`)
// are not issued in Clax, so any `request` names no open request.
import { seconds, takeSlot } from "./budget";
import { CapError } from "./errors";
import type { PromptAnswer } from "./grants";
import type { HandlerFactory } from "./host";

export const ALLOWED_EXTENSIONS = ["gif", "png", "jpg", "jpeg", "webp", "mp4", "webm", "txt", "json", "md", "docx", "pptx", "epub", "csv", "ttf", "html", "svg", "pdf", "xlsx", "zip"] as const;

/** Longest filename a page may suggest, in UTF-16 code units (downloads.d.ts). */
export const MAX_FILENAME_CHARS = 512;
/** Longest name the viewer confirms, in bytes of UTF-8. */
export const MAX_NAME_BYTES = 240;
/** Most save prompts one artifact may show in a tab within [`PROMPT_WINDOW_MS`]. */
export const PROMPTS_PER_WINDOW = 3;
export const PROMPT_WINDOW_MS = 30_000;
/** How long an object URL outlives the click that started its download. */
export const REVOKE_AFTER_MS = 60_000;

const MIME: Record<string, string> = {
  gif: "image/gif", png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", webp: "image/webp", mp4: "video/mp4", webm: "video/webm",
  txt: "text/plain", json: "application/json", md: "text/markdown", csv: "text/csv", html: "text/html", svg: "image/svg+xml", pdf: "application/pdf",
  ttf: "font/ttf", zip: "application/zip", epub: "application/epub+zip",
  docx: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  pptx: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
  xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
};

/** The name the viewer confirms: invisible (format) characters are dropped,
 * whitespace runs (tabs and newlines included) become one space, remaining
 * control characters are dropped, leading dots, path separators and spaces
 * are stripped, the other path separators become `_`, and the name is cut to
 * [`MAX_NAME_BYTES`] bytes of UTF-8 keeping its extension. */
export function sanitizeFilename(name: string): string {
  let s = name.replace(/\p{Cf}/gu, "").replace(/\s+/g, " ").replace(/\p{Cc}/gu, "").replace(/^[.\\/ ]+/, "").replace(/[\\/]/g, "_").trim();
  const enc = new TextEncoder();
  if (enc.encode(s).length > MAX_NAME_BYTES) {
    const dot = s.lastIndexOf(".");
    const ext = dot > 0 ? s.slice(dot) : "";
    let stem = [...(dot > 0 ? s.slice(0, dot) : s)];
    while (stem.length && enc.encode(stem.join("") + ext).length > MAX_NAME_BYTES) stem = stem.slice(0, -1);
    s = stem.join("") + ext;
  }
  return s;
}

/** The lowercased extension of `name`, or null when it has none. */
export function extensionOf(name: string): string | null {
  const m = /\.([A-Za-z0-9]+)$/.exec(name);
  return m ? m[1].toLowerCase() : null;
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} bytes`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/** Starts a browser download of `blob` named `name` through an object URL,
 * revoked [`REVOKE_AFTER_MS`] later. */
export function saveBlob(blob: Blob, name: string): void {
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.rel = "noopener";
  document.body.appendChild(a);
  try {
    a.click();
  } finally {
    a.remove();
    setTimeout(() => URL.revokeObjectURL(url), REVOKE_AFTER_MS);
  }
}

export const downloadsHandler: HandlerFactory = env => {
  let open = false;
  let disposed = false;
  const closed = () => new CapError("unavailable", "this view has closed");
  return {
    async call(method, args) {
      if (disposed) throw closed();
      if (method !== "save") throw new CapError("capability_removed", `downloads.${String(method)} is not part of this runtime`);
      const req = args[0];
      if (!req || typeof req !== "object") throw new CapError("bad_request", "save takes {filename, data}");
      const { filename, blob, request } = req as { filename?: unknown; blob?: unknown; request?: unknown };
      if (request !== undefined) {
        if (typeof request === "string") throw new CapError("request_unknown", "no export request was issued to this page");
        throw new CapError("bad_request", "request is the token string an export request carried");
      }
      if (typeof filename !== "string" || filename.length > MAX_FILENAME_CHARS) throw new CapError("bad_request", `filename is a string of at most ${MAX_FILENAME_CHARS} characters`);
      if (!(blob instanceof Blob) || blob.size === 0) throw new CapError("bad_request", "save takes {filename, data} with non-empty data");
      const name = sanitizeFilename(filename);
      const ext = extensionOf(name);
      if (!name || !ext || !(ALLOWED_EXTENSIONS as readonly string[]).includes(ext)) {
        throw new CapError("rejected_extension", `'${name || filename}' needs one of these extensions: ${ALLOWED_EXTENSIONS.join(", ")}`);
      }
      if (open) throw new CapError("rate_limited", "a save prompt is already open");
      const wait = takeSlot(`clax.download-prompts.v1:${env.aid}`, { perWindow: { n: PROMPTS_PER_WINDOW, ms: PROMPT_WINDOW_MS } });
      if (wait > 0) throw new CapError("rate_limited", `too many save prompts; wait ${seconds(wait)} s`);
      open = true;
      let answer: PromptAnswer;
      try {
        answer = await env.prompt({ title: "Save a file from this page?", body: `${name} (${formatBytes(blob.size)})`, allow: "Save", deny: "Cancel" });
      } finally {
        open = false;
      }
      if (disposed) throw closed();
      if (answer !== "allow") throw new CapError("declined", "the viewer did not save the file");
      saveBlob(new Blob([blob], { type: MIME[ext] }), name);
      return { status: "saved" };
    },
    dispose() {
      disposed = true;
    },
  };
};
