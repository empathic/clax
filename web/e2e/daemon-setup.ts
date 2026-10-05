// Before any worker starts: builds the daemon once, so each worker runs the
// binary rather than `cargo run`, and names it to the workers through the
// environment (CLAX_E2E_BIN). The browser tests' global setup runs this, and
// so does the time-to-usable suite (web/perf), which serves web/dist itself.
// With CLAX_TEST_BIN set (quality_gates.sh's one debug build for the run) it
// uses that binary and builds nothing: a `cargo build` here would replace
// target/debug/clax under the Rust tests running beside it.
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const repo = fileURLToPath(new URL("../..", import.meta.url));

export default function daemonSetup() {
  if (process.env.CLAX_TEST_BIN) {
    process.env.CLAX_E2E_BIN = process.env.CLAX_TEST_BIN;
    return;
  }
  const r = spawnSync("cargo", ["build", "-q", "-p", "clax-cli"], { cwd: repo, stdio: "inherit" });
  if (r.status !== 0) throw new Error(`cargo build -p clax-cli failed (${r.status ?? r.signal})`);
  process.env.CLAX_E2E_BIN = join(process.env.CARGO_TARGET_DIR ?? join(repo, "target"), "debug", "clax");
}
