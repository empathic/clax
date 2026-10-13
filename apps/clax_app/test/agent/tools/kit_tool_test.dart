import 'dart:io';

import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/kit_tool.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support.dart';

/// Reads the kit from the package's own `assets/kit/` on disk.
Future<String> _diskKit(String name) => File('assets/kit/$name').readAsString();

Map<String, Object?> _ok(ToolOutput r) {
  expect(r, isA<ToolOk>());
  return (r as ToolOk).json;
}

void main() {
  test('kit brings preact and icons, written under kit/', () async {
    final tb = toolboxWith(extra: [AddKitTool(load: (n) async => '/* $n */')]);
    final r = _ok(
      await tb.call('add_kit', {
        'parts': ['kit'],
      }),
    );
    expect(
      [for (final f in r['files']! as List) (f as Map)['path']],
      [
        'kit/preact.js',
        'kit/icons.js',
        'kit/kit.css',
        'kit/kit.js',
        'kit/LICENSES.txt',
      ],
    );
    expect(tb.state.draft.files['kit/kit.js'], '/* kit.js */');
    expect(tb.state.draft.revision, 5);
    expect(r['api'], allOf(contains('useCollection'), contains('html`')));
  });

  test('a repeated call writes nothing', () async {
    final tb = toolboxWith(extra: [AddKitTool(load: (n) async => '/* $n */')]);
    await tb.call('add_kit', {
      'parts': ['chart', 'dates'],
    });
    final rev = tb.state.draft.revision;
    final r = _ok(
      await tb.call('add_kit', {
        'parts': ['dates', 'chart', 'chart'],
      }),
    );
    expect(tb.state.draft.revision, rev);
    expect(
      [for (final f in r['files']! as List) (f as Map)['written']],
      [false, false, false],
    );
  });

  test('an unknown or empty part list is refused', () async {
    final tb = toolboxWith(extra: [AddKitTool(load: (n) async => '')]);
    expect(
      await tb.call('add_kit', {
        'parts': ['jquery'],
      }),
      isA<ToolRefused>(),
    );
    expect(await tb.call('add_kit', {'parts': []}), isA<ToolRefused>());
    expect(tb.state.draft.files.keys, ['index.html']);
  });

  test('a draft without room for the files is refused', () async {
    final files = {for (var i = 0; i < maxDraftFiles - 1; i++) 'f$i.txt': 'x'};
    final tb = toolboxWith(
      files: files,
      extra: [AddKitTool(load: (n) async => '')],
    );
    expect(
      await tb.call('add_kit', {
        'parts': ['chart', 'dates'],
      }),
      isA<ToolRefused>(),
    );
    expect(tb.state.draft.files.length, maxDraftFiles - 1);
  });

  test('every kit file is a bundled asset', () async {
    final pubspec = File('pubspec.yaml').readAsStringSync();
    expect(pubspec, contains('- assets/kit/'));
    final tb = toolboxWith(extra: [AddKitTool(load: _diskKit)]);
    final r = _ok(await tb.call('add_kit', {'parts': kitParts.keys.toList()}));
    expect((r['files']! as List).length, 7);
    // Each export of kit.js and chart.js is named in the API text.
    for (final file in ['kit.js', 'chart.js']) {
      final src = File('assets/kit/$file').readAsStringSync();
      final exports = RegExp(
        r'^export (?:async )?(?:function|const|let) (\w+)',
        multiLine: true,
      ).allMatches(src).map((m) => m.group(1)!);
      expect(exports, isNotEmpty);
      for (final name in exports) {
        expect(
          r['api'],
          matches(RegExp('\\b$name\\b')),
          reason: '$file exports $name',
        );
      }
    }
    // Each icon name the result lists is one icons.js exports.
    final icons = File('assets/kit/icons.js').readAsStringSync();
    final api = r['api']! as String;
    final listed = RegExp(r'Icon names \(.*?\): ([\w ]+)\.')
        .firstMatch(api)!
        .group(1)!
        .split(' ');
    expect(listed.length, greaterThan(80));
    for (final name in listed) {
      expect(icons, contains('export const $name ='), reason: name);
    }
  });
}
