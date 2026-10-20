import 'dart:convert';

import 'package:clax_app/bundle/content_hash.dart';
import 'package:flutter_test/flutter_test.dart';

/// The vector `crates/clax-core/src/digest.rs` checks
/// `clax_core::digest::files_digest` against: the render gate's digest is
/// [contentHash] over the files but `icon.svg`.
void main() {
  const files = {
    'index.html': '<!doctype html><title>Budget</title>',
    'app.js': 'let n = 1;\n',
    'data/café.json': '{"n":1}',
  };
  const digest =
      'sha256:f7822984f37e5debcd4ae329c8f7569f2518ded0ee8b8ca3d88266e381ff1ab2';

  test('contentHash of the files but icon.svg is the core files_digest', () {
    final hashed = {
      for (final e in files.entries)
        e.key: (
          sha256: sha256Hex(utf8.encode(e.value)),
          size: utf8.encode(e.value).length,
        ),
    };
    expect(contentHash(hashed), digest);
  });
}
