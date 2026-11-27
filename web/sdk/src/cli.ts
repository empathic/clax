// The SDK package's executable (`npx @empathic/clax-client-sdk setup`,
// scripts/build-sdk.mjs builds it to cli.js): `setup` (setup.ts) with the
// process's arguments, Node's fetch, clock and file system. Node 20 or
// later; it depends on no Clax plugin.
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { hostname } from "node:os";
import { setup, SetupError, type SetupOptions } from "./setup";

const USAGE = `Usage: npx @empathic/clax-client-sdk setup --instance <origin> --name <name> --origin <origin> [--origin <origin>]...
       [--reporters-see own|all] [--note <text>] [--write-env <path>] [--client-name <name>] [--timeout <seconds>]

Signs in to a hosted Clax for one queue request (you approve it in your
browser), asks you for a report queue taking reports from the origins,
waits for your answer, and writes CLAX_QUEUE, CLAX_QUEUE_TOKEN and
CLAX_QUEUE_KEY to the env file (default .env; made owner-only if new,
those lines replaced if present). It never prints the token or the key.
`;

/** `argv`'s options, or a usage error's message. */
export function parseArgs(argv: string[]): SetupOptions | string {
  const [cmd, ...rest] = argv;
  if (cmd !== "setup") return cmd === undefined || cmd === "--help" || cmd === "-h" ? "" : `unknown command ${cmd}`;
  const opts: SetupOptions = { instance: "", name: "", origins: [], envFile: ".env", clientName: `clax-client-sdk setup on ${hostname()}` };
  for (let i = 0; i < rest.length; i++) {
    const flag = rest[i];
    if (flag === "--help" || flag === "-h") return "";
    const value = rest[i + 1];
    if (value === undefined || value.startsWith("--")) return `${flag} takes a value`;
    i++;
    switch (flag) {
      case "--instance": opts.instance = value.replace(/\/$/, ""); break;
      case "--name": opts.name = value; break;
      case "--origin": opts.origins.push(value); break;
      case "--reporters-see":
        if (value !== "own" && value !== "all") return "--reporters-see is own or all";
        opts.reportersSee = value;
        break;
      case "--note": opts.note = value; break;
      case "--write-env": opts.envFile = value; break;
      case "--client-name": opts.clientName = value; break;
      case "--timeout": {
        const n = Number(value);
        if (!Number.isInteger(n) || n <= 0) return "--timeout is a whole number of seconds";
        opts.timeoutS = n;
        break;
      }
      default: return `unknown option ${flag}`;
    }
  }
  if (!opts.instance) return "--instance is required";
  if (!opts.name) return "--name is required";
  if (!opts.origins.length) return "at least one --origin is required";
  return opts;
}

/** Runs the command line `argv`: the process's exit code. */
export async function main(argv: string[]): Promise<number> {
  const opts = parseArgs(argv);
  if (typeof opts === "string") {
    if (opts) process.stderr.write(`${opts}\n\n`);
    process.stderr.write(USAGE);
    return opts ? 2 : 0;
  }
  try {
    await setup(opts, {
      fetch: (input, init) => fetch(input, init),
      now: () => Date.now(),
      sleep: ms => new Promise(resolve => setTimeout(resolve, ms)),
      print: line => process.stdout.write(`${line}\n`),
      env: {
        read: path => (existsSync(path) ? readFileSync(path, "utf8") : null),
        // A new file is made owner-only, and never over one made meanwhile
        // (`wx`); an existing one keeps its mode.
        write: (path, text, created) => writeFileSync(path, text, created ? { mode: 0o600, flag: "wx" } : {}),
      },
    });
    return 0;
  } catch (e) {
    const code = e instanceof SetupError ? `${e.code}: ` : "";
    process.stderr.write(`setup failed: ${code}${e instanceof Error ? e.message : String(e)}\n`);
    return 1;
  }
}

process.exitCode = await main(process.argv.slice(2));
