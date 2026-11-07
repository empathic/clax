// Tests of the runner's private copy of a project (project-copy.mjs).
//
//   node --test bazel/js/project-copy.test.mjs
import assert from 'node:assert/strict';
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { after, test } from 'node:test';
import { copyProject, inStore } from './project-copy.mjs';

const root = realpathSync(mkdtempSync(join(tmpdir(), 'project-copy-test-')));
after(() => rmSync(root, { recursive: true, force: true }));

const file = (path, text = path) => {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, text);
};
const link = (path, target) => {
  mkdirSync(dirname(path), { recursive: true });
  symlinkSync(target, path);
};

// The real tree, as bazel-bin keeps it: the project's files, one that is
// no longer declared, and rules_js's store with two packages, `pkg`
// depending on `dep`.
const real = join(root, 'bin');
const store = join(real, 'web/node_modules/.aspect_rules_js');
file(join(real, 'web/package.json'), '{}');
file(join(real, 'web/src/a.ts'), 'export const a = 1;');
file(join(real, 'web/src/gone.ts'), 'stale');
file(join(store, 'pkg@1.0.0/node_modules/pkg/index.js'), 'pkg');
file(join(store, 'dep@1.0.0/node_modules/dep/index.js'), 'dep');
link(join(store, 'pkg@1.0.0/node_modules/dep'), '../../dep@1.0.0/node_modules/dep');
link(join(real, 'web/node_modules/pkg'), '.aspect_rules_js/pkg@1.0.0/node_modules/pkg');

// The declared tree, as a test's runfiles lay it out: a link per declared
// file, the project's package links as rules_js makes them, `pkg` in the
// store as one link to its directory, and `dep` as a sandbox makes it, a
// directory of links to each file.
const declared = join(root, 'runfiles');
link(join(declared, 'web/package.json'), join(real, 'web/package.json'));
link(join(declared, 'web/src/a.ts'), join(real, 'web/src/a.ts'));
link(join(declared, 'web/node_modules/pkg'), '.aspect_rules_js/pkg@1.0.0/node_modules/pkg');
link(join(declared, 'web/node_modules/.aspect_rules_js/pkg@1.0.0/node_modules/pkg'), join(store, 'pkg@1.0.0/node_modules/pkg'));
link(join(declared, 'web/node_modules/.aspect_rules_js/pkg@1.0.0/node_modules/dep'), '../../dep@1.0.0/node_modules/dep');
link(join(declared, 'web/node_modules/.aspect_rules_js/dep@1.0.0/node_modules/dep/index.js'), join(store, 'dep@1.0.0/node_modules/dep/index.js'));
file(join(declared, 'web/skipped/big.bin'));

const project = join(root, 'copy');
mkdirSync(project);
copyProject({ declared, project, realRoot: real, skips: ['web/skipped'] });

// Every regular file under `dir`, not following links.
const filesUnder = (dir) =>
  readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    const stat = lstatSync(path);
    return stat.isDirectory() ? filesUnder(path) : stat.isFile() ? [path] : [];
  });

test("the project's own declared files are copied, and nothing else of the real tree", () => {
  const a = join(project, 'web/src/a.ts');
  assert.ok(lstatSync(a).isFile());
  assert.equal(readFileSync(a, 'utf8'), 'export const a = 1;');
  assert.ok(!existsSync(join(project, 'web/src/gone.ts')));
});

test('no package file is copied: each package is a link to its real place', () => {
  assert.deepEqual(filesUnder(join(project, 'web/node_modules')), []);
  assert.equal(realpathSync(join(project, 'web/node_modules/pkg/index.js')), join(store, 'pkg@1.0.0/node_modules/pkg/index.js'));
  assert.equal(
    realpathSync(join(project, 'web/node_modules/.aspect_rules_js/dep@1.0.0/node_modules/dep/index.js')),
    join(store, 'dep@1.0.0/node_modules/dep/index.js'),
  );
});

test('a package reaches its dependency through the store beside it', () => {
  const pkg = realpathSync(join(project, 'web/node_modules/pkg'));
  assert.equal(readFileSync(join(pkg, '../dep/index.js'), 'utf8'), 'dep');
});

test('a skipped input is left out', () => {
  assert.ok(!existsSync(join(project, 'web/skipped')));
});

test('a path is in the store only from a package down', () => {
  assert.ok(inStore('web/node_modules/.aspect_rules_js/vitest@3.2.7/node_modules/vitest'));
  assert.ok(inStore('/b/web/node_modules/.aspect_rules_js/x@1/node_modules/@scope/name/lib/a.js'));
  assert.ok(!inStore('web/node_modules/.aspect_rules_js/x@1/node_modules/@scope'));
  assert.ok(!inStore('web/node_modules/.aspect_rules_js/x@1/node_modules'));
  assert.ok(!inStore('web/node_modules/vitest'));
});
