/** Why a test daemon on `bind` is not started on `platform`, or null when
 * it is (`startDaemon`, e2e/fixtures.ts; said once for the run by
 * e2e/global-setup.ts). macOS's Application Firewall asks whether `clax` may
 * accept incoming connections when a binary it has not been told about
 * listens on any address but loopback, and each build of the test binary is
 * a new one: a test of a person on the LAN (`0.0.0.0`) would ask at every
 * run. On macOS such a test skips, saying so, unless `CLAX_E2E_LAN=1` says
 * the developer will answer it; elsewhere (CI's Linux) it runs. */
export function lanBindSkip(bind: string | undefined, platform: NodeJS.Platform, env: NodeJS.ProcessEnv): string | null {
  if (bind === undefined || bind === "127.0.0.1" || bind === "::1" || bind === "localhost") return null;
  if (platform !== "darwin" || env.CLAX_E2E_LAN === "1") return null;
  return `a daemon on ${bind} makes macOS's firewall ask whether clax may accept incoming connections; set CLAX_E2E_LAN=1 to run this test and answer it`;
}
