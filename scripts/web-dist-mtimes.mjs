#!/usr/bin/env node
// Keeps the modification times of build output whose content a build did
// not change (scripts/build-web.sh):
//
//   node scripts/web-dist-mtimes.mjs snapshot DIR... > SNAPSHOT
//   node scripts/web-dist-mtimes.mjs restore SNAPSHOT
//
// snapshot prints the path, SHA-256 and modification time (nanoseconds
// since the epoch) of every regular file under each DIR that is a
// directory, one tab-separated line each; symbolic links are neither
// listed nor followed. restore gives each listed path that is still a
// regular file with the same SHA-256 its listed modification time back, to
// the precision of a double in seconds (about a quarter of a microsecond).
// Paths are as given, relative to the working directory.
import { createHash } from "node:crypto";
import { lstatSync, readdirSync, readFileSync, statSync, utimesSync } from "node:fs";
import { join } from "node:path";

const sha256 = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");

function* files(dir) {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, e.name);
    if (e.isDirectory()) yield* files(path);
    else if (e.isFile()) yield path;
  }
}

function snapshot(dirs) {
  const lines = [];
  for (const dir of dirs) {
    let isDir = false;
    try {
      isDir = statSync(dir).isDirectory();
    } catch {}
    if (!isDir) continue;
    for (const path of files(dir)) {
      const { mtimeNs } = statSync(path, { bigint: true });
      lines.push(`${path}\t${sha256(path)}\t${mtimeNs}\n`);
    }
  }
  process.stdout.write(lines.join(""));
}

function restore(snap) {
  for (const line of readFileSync(snap, "utf8").split("\n")) {
    if (!line) continue;
    const [path, sha, ns] = line.split("\t");
    let st;
    try {
      st = lstatSync(path);
    } catch {
      continue;
    }
    if (!st.isFile() || sha256(path) !== sha) continue;
    const mtimeNs = BigInt(ns);
    const secs = Number(mtimeNs / 1_000_000_000n) + Number(mtimeNs % 1_000_000_000n) / 1e9;
    utimesSync(path, st.atime, secs);
  }
}

const [cmd, ...args] = process.argv.slice(2);
if (cmd === "snapshot") snapshot(args);
else if (cmd === "restore" && args.length === 1) restore(args[0]);
else {
  process.stderr.write("usage: web-dist-mtimes.mjs snapshot DIR... | restore SNAPSHOT\n");
  process.exit(2);
}
