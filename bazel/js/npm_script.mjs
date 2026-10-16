// Runs one of a package's scripts as `pnpm run <script>` does: the command
// from package.json, through a shell, with the package's own tools on PATH.
//
//   node npm_script.mjs [--out <dir>]... <package directory> <script> [arguments...]
//
// Extra arguments are appended to the command, as pnpm appends them. The
// command is whatever package.json says, so a Bazel target that runs a
// script cannot drift from the native one.
//
// The script runs in a private copy of the project, made here from exactly
// the files Bazel declared for this action (its runfiles) and deleted
// afterwards, however the run ends. Two reasons:
//
// - Bazel hands an action its inputs as links to where the files really
//   are. Vite's bundler and resolver are native code and follow the links;
//   the JavaScript around them does not; the two then name one file by two
//   paths. In the copy every file is a real file at one path.
// - The real directory behind the links is shared by every action and
//   keeps files whose sources are gone. Run there, a test could import a
//   module that was deleted from the checkout, and pass. The copy holds
//   what was declared and nothing else, so a result depends on that alone.
//
// `--out <dir>` names a directory the script writes, relative to the
// package: it is copied back to where Bazel expects the action's output.
import { spawn } from 'node:child_process';
import {
  chmodSync,
  constants,
  copyFileSync,
  cpSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  realpathSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, dirname, join, relative, resolve, sep } from 'node:path';
import { candidates, makeScratch } from './scratch.mjs';

const argv = process.argv.slice(2);
const outs = [];
while (argv[0] === '--out') outs.push(argv.splice(0, 2)[1]);
const [dir, script, ...extra] = argv;
if (!dir || !script) {
  console.error('usage: npm_script.mjs [--out <dir>]... <package directory> <script> [arguments...]');
  process.exit(2);
}

// The declared inputs: this program's runfiles, laid out as the workspace.
const runfiles = process.env.JS_BINARY__RUNFILES ?? process.env.RUNFILES_DIR ?? process.env.RUNFILES;
if (!runfiles) {
  console.error('error: npm_script.mjs must be run by Bazel (no runfiles directory)');
  process.exit(2);
}
const declared = join(runfiles, process.env.JS_BINARY__WORKSPACE ?? '_main');

// Where the declared files really are: found through one of them.
const manifest = join(declared, dir, 'package.json');
const real = realpathSync(manifest);
const realRoot = real.slice(0, real.length - join(dir, 'package.json').length - 1);

// Where the copy goes: the first writable place that Bazel cleans up
// itself (scratch.mjs has the order and the reasons).
const project = makeScratch(candidates({ env: process.env, cwd: process.cwd(), systemTmp: realpathSync(tmpdir()) }));
// The copy is a whole project with its packages, so it is also removed
// here however this ends: at the end, on an error, and on a signal that
// can be caught. The marker names this process, so that scripts/bazel-gc.sh
// can tell a copy still in use from one left in the machine's temporary
// directory by a process that was killed outright.
writeFileSync(join(project, '.npm-script-private'), `${process.pid}\n`);
let child = null;
let removed = false;
function cleanup() {
  if (removed) return;
  removed = true;
  rmSync(project, { recursive: true, force: true, maxRetries: 3 });
}
process.on('exit', cleanup);
for (const signal of ['SIGTERM', 'SIGINT', 'SIGHUP']) {
  process.on(signal, () => {
    // The script and what it started, then this process: 128 + the signal.
    if (child) {
      try {
        process.kill(-child.pid, 'SIGKILL');
      } catch {
        child.kill('SIGKILL');
      }
    }
    cleanup();
    process.exit(128 + ({ SIGHUP: 1, SIGINT: 2, SIGTERM: 15 })[signal]);
  });
}
const inside = (root, path) => path === root || path.startsWith(root + sep);

// Copies `from` to `to` inside the private project. A link that leads to
// the same place in the real tree, or out of it, is followed: its target's
// contents are what was declared. A link to elsewhere in the real tree is
// one of pnpm's (a package to the store, the store to a dependency): it is
// kept as a link, and its target is copied to the same place in the copy.
const pending = [[declared, project]];
const done = new Set();
function copy(from, to) {
  if (done.has(to)) return;
  const stat = lstatSync(from);
  if (stat.isSymbolicLink()) {
    const target = resolve(dirname(from), readlinkSync(from));
    const at = inside(project, to) ? relative(project, to) : null;
    const realTarget = existsSync(target) ? realpathSync(target) : target;
    if (inside(realRoot, realTarget) && relative(realRoot, realTarget) !== at) {
      const twin = join(project, relative(realRoot, realTarget));
      done.add(to);
      mkdirSync(dirname(to), { recursive: true });
      symlinkSync(relative(dirname(to), twin), to);
      pending.push([realTarget, twin]);
      return;
    }
    return copy(realTarget, to);
  }
  done.add(to);
  if (stat.isDirectory()) {
    mkdirSync(to, { recursive: true });
    for (const name of readdirSync(from)) copy(join(from, name), join(to, name));
  } else {
    mkdirSync(dirname(to), { recursive: true });
    // A clone where the file system has them (APFS, Btrfs, XFS); a copy otherwise.
    copyFileSync(from, to, constants.COPYFILE_FICLONE);
    chmodSync(to, stat.mode | 0o200);
  }
}
while (pending.length > 0) copy(...pending.pop());

const cwd = join(project, dir);
const scripts = JSON.parse(readFileSync(join(cwd, 'package.json'), 'utf8')).scripts ?? {};
const command = scripts[script];
if (!command) {
  console.error(`error: ${dir}/package.json has no "${script}" script`);
  process.exit(2);
}

// pnpm puts each direct dependency's commands in node_modules/.bin. Bazel's
// layout has the packages and not that directory, so the same commands are
// made here, from each package's own `bin` field, for the run.
const bins = join(project, '.npm-script-bin');
mkdirSync(bins);
const modules = join(cwd, 'node_modules');
const packages = existsSync(modules)
  ? readdirSync(modules).flatMap((name) =>
      name.startsWith('@') ? readdirSync(join(modules, name)).map((scoped) => join(name, scoped)) : [name],
    )
  : [];
for (const name of packages) {
  const packageJson = join(modules, name, 'package.json');
  if (!existsSync(packageJson)) continue;
  const { bin } = JSON.parse(readFileSync(packageJson, 'utf8'));
  const commands = typeof bin === 'string' ? { [name.split('/').pop()]: bin } : (bin ?? {});
  for (const [name_, file] of Object.entries(commands)) {
    const shim = join(bins, name_);
    writeFileSync(shim, `#!/bin/sh\nexec node "${join(modules, name, file)}" "$@"\n`);
    chmodSync(shim, 0o755);
  }
}

const quoted = extra.map((a) => `'${a.replaceAll("'", `'\\''`)}'`).join(' ');
// Its own process group, so that stopping it stops what it started too.
child = spawn('sh', ['-c', quoted ? `${command} ${quoted}` : command], {
  cwd,
  stdio: 'inherit',
  detached: true,
  env: {
    ...process.env,
    // The package's commands, then the Node this is running under.
    PATH: [bins, dirname(process.execPath), process.env.PATH].join(delimiter),
  },
});
const status = await new Promise((done) => {
  child.on('error', (error) => {
    console.error(`error: could not run \`${script}\` in ${dir}: ${error.message}`);
    done(1);
  });
  child.on('exit', (code, signal) => done(code ?? (signal ? 1 : 0)));
});
child = null;

// What the script built goes where Bazel expects it: beside where this
// program was started (an action starts in its output directory).
let exit = status;
if (status === 0) {
  for (const out of outs) {
    const made = join(cwd, out);
    if (!existsSync(made)) {
      console.error(`error: \`${script}\` in ${dir} did not write ${out}`);
      exit = 1;
      continue;
    }
    cpSync(made, resolve(process.cwd(), dir, out), { recursive: true });
  }
}
// The 'exit' handler removes the copy.
process.exit(exit);
