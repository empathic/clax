// The runner's private copy of a project (npm_script.mjs says why it
// makes one): exactly the declared files, with each package rules_js
// fetched linked to its real place in the store rather than copied.
import {
  chmodSync,
  constants,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  symlinkSync,
} from 'node:fs';
import { dirname, join, relative, resolve, sep } from 'node:path';

const inside = (root, path) => path === root || path.startsWith(root + sep);

// Whether a path is a package in rules_js's store, or inside one:
// .../node_modules/.aspect_rules_js/<package>@<version>/node_modules/<name>[/...].
export const inStore = (path) =>
  /(^|\/)node_modules\/\.aspect_rules_js\/[^/]+\/node_modules\/(@[^/]+\/)?[^/@][^/]*(\/|$)/.test(path.split(sep).join('/'));

// Copies the tree at `declared` (an action's runfiles, laid out as the
// workspace) into `project`. `realRoot` is the real directory the declared
// files lead to; `skips` are paths, relative to the workspace, left out.
//
// A link that leads to the same place in the real tree, or out of it, is
// followed: its target's contents are what was declared. A link to
// elsewhere in the real tree is one of pnpm's (a package to the store, the
// store to a dependency): it is kept as a link, and its target is copied to
// the same place in the copy. A package in the store, or a file in one, is
// linked to its real place instead, and nothing is copied into it.
export function copyProject({ declared, project, realRoot, skips = [] }) {
  const pending = [[declared, project]];
  const done = new Set();
  const link = (to, target) => {
    done.add(to);
    mkdirSync(dirname(to), { recursive: true });
    symlinkSync(target, to);
  };
  const copy = (from, to) => {
    if (done.has(to)) return;
    const here = relative(project, to);
    if (skips.some((skip) => here === skip || here.startsWith(skip + sep))) return;
    if (inStore(here)) return link(to, realpathSync(from));
    const stat = lstatSync(from);
    if (stat.isSymbolicLink()) {
      const target = resolve(dirname(from), readlinkSync(from));
      const at = inside(project, to) ? relative(project, to) : null;
      const realTarget = existsSync(target) ? realpathSync(target) : target;
      if (inStore(realTarget)) return link(to, realTarget);
      if (inside(realRoot, realTarget) && relative(realRoot, realTarget) !== at) {
        const twin = join(project, relative(realRoot, realTarget));
        link(to, relative(dirname(to), twin));
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
  };
  while (pending.length > 0) copy(...pending.pop());
}
