import 'dart:io';

/// Where the tests find the app's files and write their scratch files,
/// under `flutter test` (run in apps/clax_app) and under Bazel (run in the
/// runfiles tree, with TEST_SRCDIR, TEST_WORKSPACE and TEST_TMPDIR set).

final Map<String, String> _env = Platform.environment;

/// apps/clax_app: under Bazel in its runfiles, else the working directory.
String get appRoot {
  final src = _env['TEST_SRCDIR'];
  final ws = _env['TEST_WORKSPACE'];
  if (src != null && ws != null) return '$src/$ws/apps/clax_app';
  return Directory.current.path;
}

/// [rel] (a path relative to apps/clax_app, such as `assets/kit/kit.js`
/// or `../../web/contract/db.d.ts`) as a path the test can open.
String appPath(String rel) => '$appRoot/$rel';

/// A directory for files a test writes: Bazel's TEST_TMPDIR, or the
/// repository's git-ignored target/flutter-app.
Directory scratchRoot() {
  final tmp = _env['TEST_TMPDIR'];
  final dir = Directory(
    tmp ?? '${Directory.current.path}/../../target/flutter-app',
  );
  return dir..createSync(recursive: true);
}

/// The host's build of the embedded core's shared library named by
/// CLAX_EMBED_LIB as an absolute path, or null when it is not set.
String? claxEmbedLib() => _runfile('CLAX_EMBED_LIB');

/// The `clax` binary named by CLAX_TEST_BIN as an absolute path, or null
/// when it is not set.
String? claxBin() => _runfile('CLAX_TEST_BIN');

/// The file environment variable [name] names, relative to the runfiles
/// root under Bazel, as an absolute path; null when it is not set.
String? _runfile(String name) {
  final bin = _env[name];
  if (bin == null || bin.isEmpty) return null;
  if (bin.startsWith('/')) return bin;
  final src = _env['TEST_SRCDIR'];
  final ws = _env['TEST_WORKSPACE'];
  if (src != null && ws != null) return '$src/$ws/$bin';
  return File(bin).absolute.path;
}
