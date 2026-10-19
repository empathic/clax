// Writes a Flutter app's NOTICES (bazel/flutter/notices.bzl): the pub
// packages' licences (--pub "NAME\tPATH"), the engine's (--engine PATH,
// already in NOTICES form) and the Rust crates' (--crate "NAME\tPATH"), to
// --out. Packages with the same licence text share one entry; a crate with
// several licence files gets one entry per file, and one published without
// any (its Cargo.toml given instead) an entry naming its manifest's licence.
import 'dart:io';

final _separator = '\n${'-' * 80}\n';

void main(List<String> args) {
  String? out;
  String? engine;
  final pub = <String, List<String>>{};
  final crates = <String, Set<String>>{};
  String read(String path) {
    final text = File(path).readAsStringSync().replaceAll('\r\n', '\n');
    if (!path.endsWith('/Cargo.toml')) return text.trimRight();
    // A crate published without its licence text: the licence its
    // manifest names.
    final spdx = RegExp(r'^license\s*=\s*"([^"]+)"', multiLine: true)
        .firstMatch(text)
        ?.group(1);
    return spdx == null
        ? ''
        : 'Licensed under $spdx, as the crate\'s Cargo.toml says; the '
              'crate is published without the licence text.';
  }
  void add(Map<String, Iterable<String>> into, String spec) {
    final tab = spec.indexOf('\t');
    final name = spec.substring(0, tab);
    final text = read(spec.substring(tab + 1));
    if (text.isEmpty) return;
    if (into is Map<String, List<String>>) {
      (into[text] ??= []).add(name);
    } else {
      ((into as Map<String, Set<String>>)[text] ??= {}).add(name);
    }
  }

  for (var i = 0; i < args.length; i += 2) {
    final v = args[i + 1];
    switch (args[i]) {
      case '--out':
        out = v;
      case '--engine':
        engine = v;
      case '--pub':
        add(pub, v);
      case '--crate':
        // A crate's repository name is `<name>-<version>`.
        add(crates, v.replaceFirst(RegExp(r'-\d[^\t]*'), ''));
      default:
        stderr.writeln('notices_tool: unknown argument ${args[i]}');
        exit(2);
    }
  }
  final entries = <String>[];
  String entry(Iterable<String> names, String text) =>
      '${(names.toSet().toList()..sort()).join('\n')}\n\n$text';
  final pubTexts = pub.keys.toList()
    ..sort((a, b) => pub[a]!.first.compareTo(pub[b]!.first));
  for (final t in pubTexts) {
    entries.add(entry(pub[t]!, t));
  }
  if (engine != null) {
    entries.addAll(
      read(engine).split(_separator).where((e) => e.contains('\n\n')),
    );
  }
  final crateTexts = crates.keys.toList()
    ..sort((a, b) => (crates[a]!.toList()..sort()).first
        .compareTo((crates[b]!.toList()..sort()).first));
  for (final t in crateTexts) {
    entries.add(entry(crates[t]!, t));
  }
  File(out!).writeAsStringSync(entries.join(_separator));
}
