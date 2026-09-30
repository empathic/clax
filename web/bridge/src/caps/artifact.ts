// The `artifact` namespace (artifact.d.ts; `self` is its alias): `publish(html)`
// sends a complete document to the shell, which republishes it with the
// viewer's authority. The files form and the live-doc verbs are not available
// in Clax.
import type { Local } from "../capabilities";
import { CapabilityError, type Rpc } from "../rpc";

const DOCTYPE = /^[\s﻿]*<!doctype/i;
const refuse = (code: string, message: string) => Promise.reject(new CapabilityError(code, message));
const notLiveDoc = () => refuse("invalid_content", "this artifact is not a live doc");

export function artifactLocals(rpc: Pick<Rpc, "call">): Local {
  return {
    publish: (html: unknown) => {
      if (html !== null && typeof html === "object" && !Array.isArray(html)) {
        return refuse("capability_disabled", "the files form of publish is not available in Clax; publish a complete HTML page");
      }
      if (typeof html !== "string" || !DOCTYPE.test(html)) {
        return refuse("invalid_content", "publish takes a complete page that begins with <!doctype html>");
      }
      return rpc.call("artifact", "publish", [html]);
    },
    edit: notLiveDoc,
    sync: notLiveDoc,
  };
}
