import 'dart:convert';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/edit_schema.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/scripted_backend.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:flutter_test/flutter_test.dart';

/// A valid new-app result, as a backend would return it.
Map<String, Object?> newAppJson() => {
  'reply': 'Made a run tracker.',
  'title': 'Run Tracker',
  'files': [
    {'path': 'index.html', 'content': '<!doctype html><title>Runs</title>'},
  ],
  'edits': <Object?>[],
  'removed': <Object?>[],
  'db': [
    {
      'op': 'set',
      'collection': 'runs',
      'doc_id': 'first',
      'data_json': '{"km":5}',
    },
  ],
  'capabilities_json': '{"db":{}}',
  'resolve': true,
};

/// A valid change result against an artifact holding index.html and app.js.
Map<String, Object?> changeJson() => {
  'reply': 'Made the total bold.',
  'title': null,
  'files': <Object?>[],
  'edits': [
    {'path': 'index.html', 'old_str': '<b>', 'new_str': '<strong>'},
  ],
  'removed': ['app.js'],
  'db': [
    {'op': 'delete', 'collection': 'runs', 'doc_id': 'old', 'data_json': null},
  ],
  'capabilities_json': null,
  'resolve': false,
};

const existing = {'index.html', 'app.js'};

/// Parses and validates [json]; expects a [ResultInvalid] whose reason
/// contains every string in [names].
void expectRefused(
  Map<String, Object?> json,
  List<String> names, {
  RequestKind kind = RequestKind.change,
  Set<String> existingPaths = existing,
}) {
  try {
    final r = EditResult.fromJson(json);
    validateResult(r, kind: kind, existingPaths: existingPaths);
  } on ResultInvalid catch (e) {
    for (final n in names) {
      expect(e.reason, contains(n), reason: 'reason "${e.reason}" names $n');
    }
    return;
  }
  fail('expected ResultInvalid naming $names');
}

Map<String, Object?> withFiles(List<Map<String, Object?>> files) =>
    changeJson()..['files'] = files;

Map<String, Object?> withDb(List<Map<String, Object?>> ops) =>
    changeJson()..['db'] = ops;

Map<String, Object?> withPath(String path) => withFiles([
  {'path': path, 'content': 'x'},
]);

void main() {
  group('EditResult', () {
    test('round-trips through JSON', () {
      for (final j in [newAppJson(), changeJson()]) {
        final r = EditResult.fromJson(j);
        expect(r.toJson(), j);
        expect(EditResult.fromJson(r.toJson()).toJson(), j);
      }
      final r = EditResult.fromJson(newAppJson());
      expect(r.title, 'Run Tracker');
      expect(r.files.single.path, 'index.html');
      expect(r.db.single.data, {'km': 5});
      expect(r.db.single.docId, 'first');
      expect(r.capabilities, {'db': <String, Object?>{}});
      expect(r.resolve, isTrue);
      final c = EditResult.fromJson(changeJson());
      expect(c.edits.single.oldStr, '<b>');
      expect(c.edits.single.newStr, '<strong>');
      expect(c.removed, ['app.js']);
      expect(c.db.single.data, isNull);
      expect(c.capabilities, isNull);
    });

    test('valid results pass validation', () {
      validateResult(
        EditResult.fromJson(newAppJson()),
        kind: RequestKind.newApp,
        existingPaths: const {},
      );
      validateResult(
        EditResult.fromJson(changeJson()),
        kind: RequestKind.change,
        existingPaths: existing,
      );
      // An edit may target a file written in the same result.
      validateResult(
        EditResult.fromJson(
          changeJson()
            ..['files'] = [
              {'path': 'new/style.css', 'content': 'a{}'},
            ]
            ..['edits'] = [
              {'path': 'new/style.css', 'old_str': 'a', 'new_str': 'b'},
            ],
        ),
        kind: RequestKind.change,
        existingPaths: existing,
      );
    });

    test('validation_refuses_hostile_results', () {
      // Shape: the result is not what the schema describes.
      expectRefused(changeJson()..remove('reply'), ['reply']);
      expectRefused(changeJson()..['files'] = 'index.html', ['files']);
      expectRefused(
        withFiles([
          {'path': 'a.html'},
        ]),
        ['content'],
      );
      expectRefused(changeJson()..['resolve'] = 'yes', ['resolve']);

      // reply non-empty.
      expectRefused(changeJson()..['reply'] = '  ', ['reply', 'empty']);

      // New app: title and index.html.
      expectRefused(
        newAppJson()..['title'] = null,
        ['title'],
        kind: RequestKind.newApp,
        existingPaths: const {},
      );
      expectRefused(
        newAppJson()..['title'] = ' ',
        ['title'],
        kind: RequestKind.newApp,
        existingPaths: const {},
      );
      expectRefused(
        newAppJson()
          ..['files'] = [
            {'path': 'main.html', 'content': 'x'},
          ],
        ['index.html'],
        kind: RequestKind.newApp,
        existingPaths: const {},
      );

      // Paths.
      expectRefused(withPath(''), ['path', 'empty']);
      expectRefused(withPath('/etc/passwd'), ['/etc/passwd', 'relative']);
      expectRefused(withPath('../x'), ['../x', '..']);
      expectRefused(withPath('a/../../x'), ['a/../../x', '..']);
      expectRefused(withPath('./x'), ['./x', '.']);
      expectRefused(withPath('a//b'), ['a//b', 'empty segment']);
      expectRefused(withPath(r'a\b'), [jsonEncode(r'a\b'), 'backslash']);
      expectRefused(withPath('a\nb'), ['control character']);
      expectRefused(withPath('a\u0000b'), ['control character']);
      expectRefused(withPath('a\u007fb'), ['control character']);
      expectRefused(withPath('${'a' * 201}.html'), ['200 characters']);
      expectRefused(
        changeJson()
          ..['edits'] = [
            {'path': '../x', 'old_str': 'a', 'new_str': 'b'},
          ],
        ['../x'],
      );
      expectRefused(changeJson()..['removed'] = ['../x'], ['../x']);

      // removed: never index.html, only existing paths.
      expectRefused(changeJson()..['removed'] = ['index.html'], [
        'index.html',
        'removed',
      ]);
      expectRefused(changeJson()..['removed'] = ['ghost.js'], [
        'ghost.js',
        'removed',
      ]);

      // edits: only existing paths or files written in the same result.
      expectRefused(
        changeJson()
          ..['edits'] = [
            {'path': 'ghost.js', 'old_str': 'a', 'new_str': 'b'},
          ],
        ['ghost.js', 'edit'],
      );

      // Total content at most 8 MiB: a 20 MiB file.
      expectRefused(
        withFiles([
          {'path': 'big.js', 'content': 'x' * (20 * 1024 * 1024)},
        ]),
        ['8 MiB'],
      );
      // The cap counts files and edits together, in UTF-8 bytes.
      expectRefused(
        withFiles([
          {'path': 'a.js', 'content': 'é' * (2 * 1024 * 1024 + 1)},
          {'path': 'b.js', 'content': 'x' * (4 * 1024 * 1024)},
        ]),
        ['8 MiB'],
      );

      // db ops.
      Map<String, Object?> op(
        String o,
        String coll,
        String doc,
        Object? data,
      ) => {'op': o, 'collection': coll, 'doc_id': doc, 'data_json': data};
      expectRefused(withDb([op('set', 'runs/a', 'x', '{}')]), [
        'runs/a',
        'collection',
      ]);
      expectRefused(withDb([op('set', '', 'x', '{}')]), ['collection']);
      expectRefused(withDb([op('set', 'a//b/c', 'x', '{}')]), ['a//b/c']);
      expectRefused(withDb([op('set', 'runs', 'a/b', '{}')]), [
        'a/b',
        'doc_id',
      ]);
      expectRefused(withDb([op('set', 'runs', '', '{}')]), ['doc_id']);
      expectRefused(withDb([op('set', 'runs', 'x', '[1, 2]')]), [
        'data_json',
        'object',
      ]);
      expectRefused(withDb([op('update', 'runs', 'x', 'not json')]), [
        'data_json',
      ]);
      expectRefused(withDb([op('set', 'runs', 'x', null)]), [
        'data_json',
        'set',
      ]);
      expectRefused(withDb([op('delete', 'runs', 'x', '{}')]), [
        'data_json',
        'delete',
      ]);
      expectRefused(withDb([op('drop', 'runs', 'x', null)]), ['drop']);
      expectRefused(
        withDb([for (var i = 0; i < 51; i++) op('set', 'runs', 'd$i', '{}')]),
        ['50'],
      );

      // capabilities_json parses to an object.
      expectRefused(changeJson()..['capabilities_json'] = '"db"', [
        'capabilities_json',
        'object',
      ]);
      expectRefused(changeJson()..['capabilities_json'] = '{db}', [
        'capabilities_json',
      ]);
    });

    test('a subcollection path with an odd number of segments is accepted', () {
      validateResult(
        EditResult.fromJson(
          changeJson()
            ..['db'] = [
              {
                'op': 'update',
                'collection': 'runs/2026/laps',
                'doc_id': 'l1',
                'data_json': '{"s": 1}',
              },
            ],
        ),
        kind: RequestKind.change,
        existingPaths: existing,
      );
    });
  });

  group('editSchema', () {
    test('the schema has additionalProperties false on every object and lists '
        'every property as required', () {
      var objects = 0;
      void walk(Object? node, String at) {
        if (node is Map) {
          if (node['type'] == 'object') {
            objects++;
            expect(node['additionalProperties'], isFalse, reason: at);
            final props = (node['properties'] as Map).keys.toSet();
            expect(
              (node['required'] as List).toSet(),
              props,
              reason: '$at lists every property as required',
            );
          }
          node.forEach((k, v) => walk(v, '$at/$k'));
        } else if (node is List) {
          for (var i = 0; i < node.length; i++) {
            walk(node[i], '$at[$i]');
          }
        }
      }

      walk(editSchema, '#');
      expect(objects, 4, reason: 'the result, a file, an edit, a db op');
      // The schema is plain JSON.
      expect(jsonDecode(jsonEncode(editSchema)), editSchema);
    });

    test('a valid result conforms to the schema\'s top-level keys', () {
      expect(
        newAppJson().keys.toSet(),
        (editSchema['required'] as List).toSet(),
      );
    });
  });

  group('ScriptedBackend', () {
    test('returns its script in order and records the requests', () async {
      final ok = EditResult.fromJson(newAppJson());
      const err = AgentError('rate limited', retryable: true);
      final b = ScriptedBackend([ok, err]);
      expect(b.name, 'scripted');
      const req = AgentRequest(
        kind: RequestKind.newApp,
        model: 'claude-haiku-5-5',
        systemPrompt: 'sys',
        userText: 'a run tracker',
      );
      expect(await b.run(req), same(ok));
      await expectLater(
        b.run(req),
        throwsA(
          isA<AgentError>()
              .having((e) => e.message, 'message', 'rate limited')
              .having((e) => e.retryable, 'retryable', isTrue),
        ),
      );
      expect(b.seen, [req, req]);
      expect(() => b.run(req), throwsStateError);
    });
  });

  group('NoTimings', () {
    test('records nothing', () {
      const t = NoTimings();
      t.start('say-usable');
      expect(t.stop('say-usable', extra: {'id': 'a'}), isNull);
      expect(t.recent(10), isEmpty);
    });
  });
}
