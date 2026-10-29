// The build steps a local run takes before it starts a server (demo/local.ts,
// demo/clax-dev-local.ts): each step prints one line, and its output goes
// to a build log in the run directory, so the terminal keeps the steps and
// what the run prints once it is up. A step that fails rejects with the
// end of that log.

import { spawn, type ChildProcess } from "node:child_process";
import { openSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { buildWorker } from "./worker.ts";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

function exited(child: ChildProcess): Promise<number | null> {
  return new Promise((ok) => {
    if (child.exitCode !== null || child.signalCode !== null) ok(child.exitCode);
    else child.once("exit", (code) => ok(code));
  });
}

export class BuildLog {
  readonly path: string;
  private readonly fd: number;

  /** Appends to `<dir>/build.log`; `dir` must exist. */
  constructor(dir: string) {
    this.path = join(dir, "build.log");
    this.fd = openSync(this.path, "a");
  }

  private failed(what: string): Error {
    const tail = readFileSync(this.path, "utf8").trimEnd().split("\n").slice(-40).join("\n");
    return new Error(`${what} failed; the end of ${this.path}:\n${tail}`);
  }

  /** Runs `cmd` in the checkout, its output to the log. */
  async step(what: string, cmd: string, args: string[], verb = "Building"): Promise<void> {
    console.log(`${verb} ${what}…`);
    const code = await exited(spawn(cmd, args, { cwd: ROOT, stdio: ["ignore", this.fd, this.fd] }));
    if (code !== 0) throw this.failed(`${verb} ${what}`);
  }

  /** Runs `cmd` in the checkout, its stderr to the log, and resolves with
   * the last line of its stdout (`scripts/run.sh bin` prints the binary's
   * path last). */
  async output(what: string, cmd: string, args: string[]): Promise<string> {
    console.log(`Building ${what}…`);
    const child = spawn(cmd, args, { cwd: ROOT, stdio: ["ignore", "pipe", this.fd] });
    let out = "";
    child.stdout?.on("data", (d: Buffer) => (out += d.toString()));
    if ((await exited(child)) !== 0) throw this.failed(`Building ${what}`);
    return out.trim().split("\n").pop() ?? "";
  }

  /** The Worker's packages (wrangler) from their lockfile, skipped when
   * node_modules already matches it (scripts/pnpm-install-stamped.sh). */
  workerPackages(): Promise<void> {
    return this.step("the Worker's packages (wrangler)", join(ROOT, "scripts/pnpm-install-stamped.sh"), ["crates/clax-worker", "--silent"], "Installing");
  }

  /** The web UI (scripts/run.sh web: Bazel when it is installed). */
  web(): Promise<void> {
    return this.step("the web UI", join(ROOT, "scripts/run.sh"), ["web"]);
  }

  /** The Worker (crates/clax-worker/build.sh), after the web UI it embeds. */
  async worker(): Promise<void> {
    console.log("Building the Worker…");
    await buildWorker(this.fd).catch(() => {
      throw this.failed("Building the Worker");
    });
  }
}
