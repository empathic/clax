// The `assets` namespace (assets.d.ts): argument checks in the page; the
// shell checks the type, size, and body, and uploads with the owner's token.
import type { Local } from "../capabilities";
import { CapabilityError, type Rpc } from "../rpc";

const refuse = (message: string) => Promise.reject(new CapabilityError("invalid_request", message));

/** The asset ID in `ref`: the ID itself, or the part of a `/_blob/<id>` URL after that prefix. */
const REF = /^(?:.*\/_blob\/)?([0-9A-Za-z]{1,64})(?:[?#].*)?$/s;

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
