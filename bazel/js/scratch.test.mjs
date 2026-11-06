// Tests of where the runner puts its private copy (scratch.mjs).
//
//   node --test bazel/js/scratch.test.mjs
import assert from 'node:assert/strict';
import { chmodSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { after, test } from 'node:test';
import { candidates, makeScratch, removedByBazel, sandboxOf } from './scratch.mjs';

const root = mkdtempSync(join(tmpdir(), 'scratch-test-'));
after(() => {
  chmodSync(join(root, 'readonly'), 0o755);
  rmSync(root, { recursive: true, force: true });
});
const dir = (name, mode) => {
  const path = join(root, name);
  mkdirSync(path, { recursive: true });
  if (mode !== undefined) chmodSync(path, mode);
  return path;
};
const writable = dir('writable');
const other = dir('other');
// Stands in for a sandbox directory on a read-only mount (Linux).
const readonly = dir('readonly', 0o555);

test('a sandbox directory is recognised from the working directory', () => {
  const linux = '/home/runner/.cache/bazel-ci/root/b6cf/sandbox/linux-sandbox/2339';
  assert.equal(sandboxOf(`${linux}/execroot/_main/bazel-out/k8-fastbuild/bin`), linux);
  const mac = '/Users/a/Library/Caches/bazel/_bazel_a/bd38/sandbox/darwin-sandbox/13';
  assert.equal(sandboxOf(`${mac}/execroot/_main/bazel-out/darwin_arm64-fastbuild/bin`), mac);
  assert.equal(sandboxOf(mac), mac);
  assert.equal(sandboxOf('/Users/a/Library/Caches/bazel/_bazel_a/bd38/execroot/_main'), null);
  assert.equal(sandboxOf('/home/runner/sandbox/notes'), null);
});

test("a test's own temporary directory comes first", () => {
  const places = candidates({
    env: { TEST_TMPDIR: '/t/test-tmp' },
    cwd: '/o/sandbox/linux-sandbox/7/execroot/_main',
    systemTmp: '/tmp',
  });
  assert.deepEqual(places, ['/t/test-tmp', '/o/sandbox/linux-sandbox/7', '/tmp']);
});

test('an action tries its sandbox directory, then the temporary directory', () => {
  const places = candidates({ env: {}, cwd: '/o/sandbox/darwin-sandbox/7/execroot/_main', systemTmp: '/tmp' });
  assert.deepEqual(places, ['/o/sandbox/darwin-sandbox/7', '/tmp']);
});

test('outside a sandbox there is only the temporary directory', () => {
  assert.deepEqual(candidates({ env: {}, cwd: '/o/execroot/_main', systemTmp: '/tmp' }), ['/tmp']);
});

test('a writable sandbox directory is used (macOS)', () => {
  const made = makeScratch([writable, other]);
  assert.equal(dirname(made), writable);
  assert.ok(existsSync(made));
});

test('a read-only sandbox directory is passed over for the next place (Linux)', () => {
  const made = makeScratch([readonly, other]);
  assert.equal(dirname(made), other);
});

test('a place that is missing, or is a file, is passed over', () => {
  const file = join(root, 'a-file');
  writeFileSync(file, '');
  assert.equal(dirname(makeScratch([join(root, 'missing'), file, other])), other);
});

test('when no place will do, the error says why', () => {
  assert.throws(() => makeScratch([readonly]), /EACCES|EROFS|EPERM/);
  assert.throws(() => makeScratch([]), /no place/);
});

test('each call makes a directory of its own', () => {
  assert.notEqual(makeScratch([writable]), makeScratch([writable]));
});

test("a copy inside the test's temporary directory is left for Bazel to remove", () => {
  assert.equal(removedByBazel('/t/test-tmp/npm-script-ab12', { TEST_TMPDIR: '/t/test-tmp' }), true);
  assert.equal(removedByBazel('/t/test-tmp/npm-script-ab12', { TEST_TMPDIR: '/t/test-tmp/' }), true);
  // A sibling sharing only the prefix, a sandbox directory, or no test at all.
  assert.equal(removedByBazel('/t/test-tmp-other/npm-script-ab12', { TEST_TMPDIR: '/t/test-tmp' }), false);
  assert.equal(removedByBazel('/o/sandbox/darwin-sandbox/7/npm-script-ab12', { TEST_TMPDIR: '/t/test-tmp' }), false);
  assert.equal(removedByBazel('/tmp/npm-script-ab12', {}), false);
});
