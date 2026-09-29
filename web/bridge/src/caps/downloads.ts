// The `downloads` namespace (downloads.d.ts): the page's data becomes a Blob
// here and the shell asks the viewer before saving it. An ArrayBuffer is
// transferred (detached after the call), a view is copied, and a Blob is
// handed over as-is.
import type { Local } from "../capabilities";
import { CapabilityError, type Rpc } from "../rpc";

/** Longest filename a page may suggest, in UTF-16 code units (downloads.d.ts). */
export const MAX_FILENAME_CHARS = 512;

const refuse = (code: string, message: string) => Promise.reject(new CapabilityError(code, message));

/** `d` as a Blob, or null when it is not a form `save` accepts. */
function toBlob(d: unknown): Blob | null {
  if (typeof d === "string") return new Blob([d]);
  if (d instanceof Blob) return d;
  if (d instanceof ArrayBuffer) {
    if (d.byteLength === 0) return new Blob([]);
    // Moves the bytes out, detaching the page's buffer, as a transfer does.
    return new Blob([structuredClone(d, { transfer: [d] })]);
  }
  if (ArrayBuffer.isView(d)) return new Blob([d as BlobPart]);
  return null;
}

export function downloadsLocals(rpc: Pick<Rpc, "call">): Local {
  return {
    save: (req: unknown) => {
      if (!req || typeof req !== "object") return refuse("bad_request", "save takes {filename, data}");
      const r = req as { filename?: unknown; data?: unknown; request?: unknown };
      if (r.request !== undefined) {
        return typeof r.request === "string"
          ? refuse("request_unknown", "no export request was issued to this page")
          : refuse("bad_request", "request is the token string an export request carried");
      }
      if (typeof r.filename !== "string" || r.filename.length > MAX_FILENAME_CHARS) {
        return refuse("bad_request", `filename is a string of at most ${MAX_FILENAME_CHARS} characters`);
      }
      const blob = toBlob(r.data);
      if (!blob) return refuse("bad_request", "data is a string, Blob, ArrayBuffer, or typed array");
      if (blob.size === 0) return refuse("bad_request", "data is empty (or a detached buffer)");
      return rpc.call("downloads", "save", [{ filename: r.filename, blob }]);
    },
  };
}
