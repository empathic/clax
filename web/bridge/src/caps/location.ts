// The `location` namespace's page side (clax-extensions.d.ts
// `ClaxExtensions.Device.Location`): `current()` is a plain shell call;
// `watch(onUpdate, {minDistance, onError})` starts a watch under a page-made
// ID (`location.watch`, from the person's tap), delivers the shell's
// `location`/`update` pushes for it, and ends on its `location`/`end` push
// (`stopped` when the app left the screen) or a refused start, reported to
// `onError` once. The returned function stops it (`location.unwatch`).
import type { Local } from "../capabilities";
import { CapabilityError, type Rpc, isCapabilityError } from "../rpc";

export function locationLocals(rpc: Pick<Rpc, "call" | "on">): Local {
  let seq = 0;
  return {
    watch: (onUpdate: unknown, opts: unknown = {}) => {
      if (typeof onUpdate !== "function") throw new TypeError("location.watch takes a function called with each position");
      const o = (opts && typeof opts === "object" ? opts : {}) as { minDistance?: unknown; onError?: unknown };
      const onError = typeof o.onError === "function" ? (o.onError as (e: CapabilityError) => void) : null;
      const watch = `w${++seq}`;
      let dead = false;
      const off: (() => void)[] = [];
      const end = (e: unknown) => {
        if (dead) return;
        dead = true;
        for (const f of off) f();
        const err = isCapabilityError(e) ? e : new CapabilityError("unavailable", String(e));
        if (onError) { try { onError(err); } catch (x) { reportError(x); } } else reportError(err);
      };
      off.push(rpc.on("location", "update", d => {
        const x = d as { watch?: unknown; value?: unknown };
        if (dead || x.watch !== watch) return;
        try { (onUpdate as (p: unknown) => void)(x.value); } catch (e) { reportError(e); }
      }));
      off.push(rpc.on("location", "end", d => {
        const x = d as { watch?: unknown; code?: unknown; message?: unknown };
        if (x.watch === watch) end(new CapabilityError(typeof x.code === "string" ? x.code : "unavailable", typeof x.message === "string" ? x.message : "location stopped"));
      }));
      rpc.call("location", "watch", [{ watch, minDistance: o.minDistance }]).catch(end);
      return () => {
        if (dead) return;
        dead = true;
        for (const f of off) f();
        void rpc.call("location", "unwatch", [{ watch }]).catch(() => {});
      };
    },
  };
}
