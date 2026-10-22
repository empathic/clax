// The `assets` namespace (assets.d.ts): argument checks in the page; the
// origin's client (origin-client.ts) checks the type, size and body against
// the contract and uploads to the page's own site, which checks them again
// and decides (only the owner's site session may upload or delete).
import type { Local } from "../capabilities";
import { CapabilityError, type Rpc } from "../rpc";

const refuse = (message: string) => Promise.reject(new CapabilityError("invalid_request", message));

/** The asset ID in `ref`: the ID itself, or the last part of a
 * `/_clax/blob/<id>` URL (or of a legacy artifact's `/_blob/<id>`, or of
 * `/_blob/<artifact ID>/<id>`). */
const REF = /^(?:.*\/(?:_clax\/blob|_blob(?:\/[0-9a-z]{12})?)\/)?([0-9A-Za-z]{1,64})(?:[?#].*)?$/s;

export function assetsLocals(rpc: Pick<Rpc, "call">): Local {
  return {
    upload: (blob: unknown, options?: unknown) => {
      if (!(blob instanceof Blob) || blob.size === 0) return refuse("upload takes a non-empty Blob or File");
      if (options !== undefined && (options === null || typeof options !== "object")) return refuse("options is {type?}");
      const type = (options as { type?: unknown } | undefined)?.type;
      if (type !== undefined && typeof type !== "string") return refuse("options.type is a string");
      return rpc.call("assets", "upload", [{ blob, type }]);
    },
    delete: (ref: unknown) => {
      const m = typeof ref === "string" ? REF.exec(ref) : null;
      if (!m) return refuse("delete takes an asset ID or its URL");
      return rpc.call("assets", "delete", [m[1]]);
    },
  };
}
