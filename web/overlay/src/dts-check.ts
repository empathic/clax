// Holds web/overlay/overlay.d.ts (served as /_clax/overlay.d.ts) equal to
// the overlay's own exports (main.ts): typecheck fails on any difference.
import type * as Dts from "../overlay.d.ts";
import type * as Main from "./main";

/** True only when `A` and `B` are each assignable to the other. */
type Same<A, B> = [A] extends [B] ? ([B] extends [A] ? true : false) : false;
/** True only when `A` and `B` are the same type, optional fields too. */
type Equal<A, B> = (<T>() => T extends A ? 1 : 2) extends (<T>() => T extends B ? 1 : 2) ? true : false;

/** The report as the API contract generates it (web/sdk/src/contract.gen.ts):
 * the served copy may leave out no field. */
export const EQUAL: [Equal<Dts.Report, Main.Report>] = [true];

export const SAME: [
  Same<Dts.OverlayOptions, Main.OverlayOptions>,
  Same<Dts.OverlayHandle, Main.OverlayHandle>,
  Same<Dts.OverlayEvents, Main.OverlayEvents>,
  Same<Dts.Report, Main.Report>,
  Same<Dts.HostContext, Main.HostContext>,
  Same<Dts.Mode, Main.Mode>,
  Same<Dts.Recorder, Main.Recorder>,
  Same<typeof Dts.overlay, typeof Main.overlay>,
  Same<typeof Dts.diagnostics, typeof Main.diagnostics>,
  Same<typeof Dts.CONTEXT_TIMEOUT_MS, typeof Main.CONTEXT_TIMEOUT_MS>,
  Same<typeof Dts.forgetReporter, typeof Main.forgetReporter>,
  Same<Dts.ReporterRef, Main.ReporterRef>,
  Same<Window["Clax"], Pick<typeof Main, "overlay" | "diagnostics" | "CONTEXT_TIMEOUT_MS" | "forgetReporter">>,
] = [true, true, true, true, true, true, true, true, true, true, true, true, true];
