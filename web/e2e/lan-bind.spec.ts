// Which daemons `startDaemon` skips before they listen (`lanBindSkip`): on
// macOS one on any address but loopback, unless CLAX_E2E_LAN=1, since its
// firewall asks about each new test binary that listens there; on Linux
// (CI) none.
import { expect, test } from "./fixtures";
import { lanBindSkip } from "./lan-bind";

test("a LAN daemon skips on macOS unless CLAX_E2E_LAN=1, and runs elsewhere", () => {
  for (const bind of [undefined, "127.0.0.1", "::1", "localhost"]) expect(lanBindSkip(bind, "darwin", {})).toBeNull();
  expect(lanBindSkip("0.0.0.0", "darwin", {})).toContain("CLAX_E2E_LAN=1");
  expect(lanBindSkip("0.0.0.0", "darwin", { CLAX_E2E_LAN: "0" })).toContain("CLAX_E2E_LAN=1");
  expect(lanBindSkip("0.0.0.0", "darwin", { CLAX_E2E_LAN: "1" })).toBeNull();
  expect(lanBindSkip("0.0.0.0", "linux", {})).toBeNull();
});
