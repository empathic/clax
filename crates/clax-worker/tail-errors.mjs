// Prints the Worker's errors from a file of `wrangler tail --format json`
// output, credentials masked (deploy-lib.mjs tailErrors): what
// scripts/cloudflare-setup.sh shows when a smoke fails, since a step that
// fails inside the Worker says why only in the Worker's log.
//
//   node tail-errors.mjs <file>
import { readFileSync } from "node:fs";
import { tailErrors } from "./deploy-lib.mjs";

const file = process.argv[2];
if (!file) {
  console.error("usage: node tail-errors.mjs <file of wrangler tail --format json output>");
  process.exit(2);
}
const lines = tailErrors(readFileSync(file, "utf8"));
if (lines.length === 0) console.log("(the Worker logged no error or warning, and no invocation failed)");
for (const l of lines) console.log(l);
