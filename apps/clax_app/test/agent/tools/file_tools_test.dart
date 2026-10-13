import 'dart:convert';

import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support.dart';

Matcher refused(Object message) =>
    isA<ToolRefused>().having((e) => e.message, 'message', message);

Map<String, Object?> okJson(ToolOutput r) {
  expect(r, isA<ToolOk>());
  return (r as ToolOk).json;
}

void main() {
  group('str_replace', () {
    test('needs exactly one match', () async {
      final tb = toolboxWith(files: {'index.html': '<p>a</p><p>a</p>'});
      final r = await tb.call('str_replace', {
        'path': 'index.html',
        'old_str': '<p>a</p>',
        'new_str': 'x',
      });
      expect(r, refused(contains('2 times')));
      expect(tb.state.draft.files['index.html'], '<p>a</p><p>a</p>');
      expect(tb.state.draft.revision, 0);
    });

    test('a missing old_str is named', () async {
      final tb = toolboxWith();
      final r = await tb.call('str_replace', {
        'path': 'index.html',
        'old_str': '<b>',
        'new_str': 'x',
      });
      expect(r, refused(contains('was not found')));
    });

    test('replaces the one match and bumps the revision', () async {
      final tb = toolboxWith(files: {'index.html': '<p>a</p><p>b</p>'});
      final r = await tb.call('str_replace', {
        'path': 'index.html',
        'old_str': '<p>b</p>',
        'new_str': '<p>c</p>',
      });
      expect(okJson(r), {'path': 'index.html', 'replaced': 1});
      expect(tb.state.draft.files['index.html'], '<p>a</p><p>c</p>');
      expect(tb.state.draft.revision, 1);
    });

    test('refuses a file the draft does not have', () async {
      final tb = toolboxWith();
      final r = await tb.call('str_replace', {
        'path': 'app.js',
        'old_str': 'a',
        'new_str': 'b',
      });
      expect(r, refused(contains('no file "app.js"')));
    });

    test('refuses an empty old_str', () async {
      final tb = toolboxWith();
      final r = await tb.call('str_replace', {
        'path': 'index.html',
        'old_str': '',
        'new_str': 'b',
      });
      expect(r, refused(contains('old_str is empty')));
    });

    test('refuses non-string input, since it is streamed unchecked', () async {
      final tb = toolboxWith();
      final r = await tb.call('str_replace', {
        'path': 'index.html',
        'old_str': 'hi',
        'new_str': 3,
      });
      expect(r, refused(contains('new_str must be a string')));
    });

    test('refuses a result over 2 MiB', () async {
      final tb = toolboxWith(files: {'index.html': 'x${'a' * (2 << 20)}'});
      final r = await tb.call('str_replace', {
        'path': 'index.html',
        'old_str': 'x',
        'new_str': 'yy',
      });
      expect(r, refused(contains('2 MiB')));
    });
  });

  group('icon.svg', () {
    const good =
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
        '<rect width="1024" height="1024" fill="#f80"/></svg>';

    test('write_file takes a good icon', () async {
      final tb = toolboxWith();
      final r = await tb.call('write_file', {
        'path': 'icon.svg',
        'content': good,
      });
      expect(okJson(r)['created'], true);
      expect(tb.state.draft.files['icon.svg'], good);
    });

    test(
      'write_file refuses an icon with a script, and keeps the draft',
      () async {
        final tb = toolboxWith();
        final r = await tb.call('write_file', {
          'path': 'icon.svg',
          'content': good.replaceFirst('</svg>', '<script>x()</script></svg>'),
        });
        expect(r, refused(contains('<script>')));
        expect(tb.state.draft.files.containsKey('icon.svg'), isFalse);
      },
    );

    test(
      'str_replace refuses an edit that makes the icon not square',
      () async {
        final tb = toolboxWith(
          files: {'index.html': '<p>hi</p>', 'icon.svg': good},
        );
        final r = await tb.call('str_replace', {
          'path': 'icon.svg',
          'old_str': '0 0 1024 1024',
          'new_str': '0 0 1024 500',
        });
        expect(r, refused(contains('not square')));
        expect(tb.state.draft.files['icon.svg'], good);
      },
    );

    test('another .svg file is not checked as an icon', () async {
      final tb = toolboxWith();
      final r = await tb.call('write_file', {
        'path': 'img/logo.svg',
        'content': '<svg><script/></svg>',
      });
      expect(r, isA<ToolOk>());
    });
  });

  group('write_file', () {
    test('creates and replaces, reporting bytes and lines', () async {
      final tb = toolboxWith();
      expect(
        okJson(
          await tb.call('write_file', {'path': 'app.js', 'content': 'a\nbé\n'}),
        ),
        {'path': 'app.js', 'bytes': 6, 'lines': 2, 'created': true},
      );
      expect(
        okJson(await tb.call('write_file', {'path': 'app.js', 'content': 'c'})),
        {'path': 'app.js', 'bytes': 1, 'lines': 1, 'created': false},
      );
      expect(tb.state.draft.files['app.js'], 'c');
      expect(tb.state.draft.revision, 2);
    });

    for (final bad in ['../x', '/etc/passwd', 'a/./b', 'a//b', r'a\b', '']) {
      test('refuses the path ${jsonEncode(bad)}', () async {
        final tb = toolboxWith();
        final r = await tb.call('write_file', {'path': bad, 'content': 'x'});
        expect(r, isA<ToolRefused>());
        expect(tb.state.draft.files.keys, ['index.html']);
        expect(tb.state.draft.revision, 0);
      });
    }

    test('refuses a file over 2 MiB', () async {
      final tb = toolboxWith();
      final r = await tb.call('write_file', {
        'path': 'big.txt',
        'content': 'é' * ((1 << 20) + 1),
      });
      expect(r, refused(contains('2 MiB')));
      expect(tb.state.draft.files.containsKey('big.txt'), isFalse);
    });

    test('refuses a 65th file but replaces one of 64', () async {
      final files = {
        'index.html': 'x',
        for (var i = 1; i < 64; i++) 'f$i.txt': '$i',
      };
      final tb = toolboxWith(files: files);
      expect(
        await tb.call('write_file', {'path': 'f65.txt', 'content': 'x'}),
        refused(contains('64 files')),
      );
      expect(
        await tb.call('write_file', {'path': 'f1.txt', 'content': 'y'}),
        isA<ToolOk>(),
      );
    });

    test(
      'refuses non-string content, since it is streamed unchecked',
      () async {
        final tb = toolboxWith();
        expect(
          await tb.call('write_file', {'path': 'a.txt', 'content': 12}),
          refused(contains('content must be a string')),
        );
        expect(
          await tb.call('write_file', {'path': 'a.txt'}),
          refused(contains('content must be a string')),
        );
        expect(
          await tb.call('write_file', {
            'path': 'a.txt',
            'content': 'x',
            'extra': 1,
          }),
          refused(contains('"extra"')),
        );
      },
    );
  });

  group('delete_file', () {
    test('never deletes index.html', () async {
      final tb = toolboxWith();
      expect(
        await tb.call('delete_file', {'path': 'index.html'}),
        refused(contains('index.html')),
      );
      expect(tb.state.draft.files.keys, ['index.html']);
    });

    test('deletes another file', () async {
      final tb = toolboxWith(files: {'index.html': 'x', 'a.css': 'y'});
      expect(okJson(await tb.call('delete_file', {'path': 'a.css'})), {
        'path': 'a.css',
        'deleted': true,
      });
      expect(tb.state.draft.files.keys, ['index.html']);
      expect(tb.state.draft.revision, 1);
      expect(
        await tb.call('delete_file', {'path': 'a.css'}),
        refused(contains('no file "a.css"')),
      );
    });
  });

  group('read_file', () {
    final text = [for (var i = 1; i <= 2000; i++) 'line $i'].join('\n');

    test('reads a window of lines', () async {
      final tb = toolboxWith(files: {'index.html': text});
      expect(
        okJson(
          await tb.call('read_file', {
            'path': 'index.html',
            'from_line': 10,
            'max_lines': 3,
          }),
        ),
        {
          'path': 'index.html',
          'from_line': 10,
          'lines': ['line 10', 'line 11', 'line 12'],
          'total_lines': 2000,
          'truncated': true,
        },
      );
    });

    test('reads at most 1,500 lines a call', () async {
      final tb = toolboxWith(files: {'index.html': text});
      final j = okJson(
        await tb.call('read_file', {
          'path': 'index.html',
          'from_line': null,
          'max_lines': null,
        }),
      );
      expect(j['from_line'], 1);
      expect((j['lines'] as List).length, 1500);
      expect(j['truncated'], isTrue);
      final rest = okJson(
        await tb.call('read_file', {
          'path': 'index.html',
          'from_line': 1501,
          'max_lines': 9999,
        }),
      );
      expect((rest['lines'] as List).length, 500);
      expect((rest['lines'] as List).last, 'line 2000');
      expect(rest['truncated'], isFalse);
    });

    test('reads at most 64 KiB a call', () async {
      final long = [for (var i = 0; i < 100; i++) 'x' * 1000].join('\n');
      final tb = toolboxWith(files: {'index.html': long});
      final j = okJson(
        await tb.call('read_file', {
          'path': 'index.html',
          'from_line': null,
          'max_lines': null,
        }),
      );
      expect((j['lines'] as List).length, 65);
      expect(j['truncated'], isTrue);
    });

    test('cuts one line longer than 64 KiB and says so', () async {
      final tb = toolboxWith(files: {'index.html': 'é' * 40000});
      final j = okJson(
        await tb.call('read_file', {
          'path': 'index.html',
          'from_line': null,
          'max_lines': null,
        }),
      );
      final line = (j['lines'] as List).single as String;
      expect(utf8.encode(line).length, lessThanOrEqualTo(64 * 1024));
      expect(j['truncated'], isTrue);
      expect(j['note'], contains('line 1'));
    });

    test('a trailing newline is not another line', () async {
      final tb = toolboxWith(files: {'index.html': 'a\nb\n'});
      final j = okJson(
        await tb.call('read_file', {
          'path': 'index.html',
          'from_line': null,
          'max_lines': null,
        }),
      );
      expect(j['lines'], ['a', 'b']);
      expect(j['total_lines'], 2);
    });

    test('refuses a missing file and a line past the end', () async {
      final tb = toolboxWith();
      expect(
        await tb.call('read_file', {
          'path': 'x.js',
          'from_line': null,
          'max_lines': null,
        }),
        refused(contains('no file "x.js"')),
      );
      expect(
        await tb.call('read_file', {
          'path': 'index.html',
          'from_line': 5,
          'max_lines': null,
        }),
        refused(contains('1 line')),
      );
      expect(
        await tb.call('read_file', {
          'path': 'index.html',
          'from_line': 0,
          'max_lines': null,
        }),
        refused(contains('from_line')),
      );
    });
  });

  group('list_files', () {
    test('lists the draft with its meta', () async {
      final tb = toolboxWith(
        files: {'index.html': 'a\nb', 'a.css': ''},
        capabilities: {'db': {}},
      );
      expect(okJson(await tb.call('list_files', {})), {
        'base_version': 3,
        'files': [
          {'path': 'a.css', 'bytes': 0, 'lines': 0},
          {'path': 'index.html', 'bytes': 3, 'lines': 2},
        ],
        'title': 'Runs',
        'capabilities_json': '{"db":{}}',
      });
    });
  });

  group('set_meta', () {
    test('sets the title and capabilities; null keeps them', () async {
      final tb = toolboxWith(kind: RequestKind.newApp, title: null);
      expect(
        okJson(
          await tb.call('set_meta', {
            'title': 'Weekly runs',
            'capabilities_json': '{"db": {}}',
          }),
        ),
        {'title': 'Weekly runs', 'capabilities_json': '{"db":{}}'},
      );
      expect(
        okJson(
          await tb.call('set_meta', {'title': null, 'capabilities_json': null}),
        ),
        {'title': 'Weekly runs', 'capabilities_json': '{"db":{}}'},
      );
      expect(tb.state.draft.title, 'Weekly runs');
      expect(tb.state.draft.capabilities, {'db': <String, Object?>{}});
      expect(tb.state.draft.revision, 1);
    });

    test('rejects invalid JSON and a non-object', () async {
      final tb = toolboxWith();
      expect(
        await tb.call('set_meta', {'title': null, 'capabilities_json': '{db'}),
        refused(contains('not valid JSON')),
      );
      expect(
        await tb.call('set_meta', {'title': null, 'capabilities_json': '[1]'}),
        refused(contains('JSON object')),
      );
      expect(
        await tb.call('set_meta', {'title': '  ', 'capabilities_json': null}),
        refused(contains('title')),
      );
      expect(tb.state.draft.revision, 0);
    });
  });

  group('specs', () {
    test('write_file and str_replace stream eagerly and are not strict', () {
      final tb = toolboxWith();
      final byName = {for (final s in tb.specs) s.name: s.toMessagesApi()};
      for (final name in ['write_file', 'str_replace']) {
        expect(byName[name]!['eager_input_streaming'], isTrue);
        expect(byName[name]!.containsKey('strict'), isFalse);
      }
      for (final name in [
        'list_files',
        'read_file',
        'delete_file',
        'set_meta',
      ]) {
        final j = byName[name]!;
        expect(j['strict'], isTrue, reason: name);
        expect(j.containsKey('eager_input_streaming'), isFalse);
        final schema = j['input_schema'] as Map<String, Object?>;
        expect(schema['additionalProperties'], isFalse, reason: name);
        expect(
          (schema['properties'] as Map).keys.toSet(),
          containsAll(schema['required'] as List),
          reason: '$name: required names only its own properties',
        );
      }
    });

    test('the MCP form names inputSchema', () {
      final spec = fileTools().first.spec;
      expect(spec.toMcp().keys, ['name', 'description', 'inputSchema']);
    });
  });
}
