// Page-side members of each capability; the rest are plain shell calls.
import type { CapabilityName, Local } from "../capabilities";
import type { Rpc } from "../rpc";

export function localsFor(name: CapabilityName, rpc: Rpc, config: unknown): Local {
  void rpc;
  void config;
  switch (name) {
    default:
      return {};
  }
}
