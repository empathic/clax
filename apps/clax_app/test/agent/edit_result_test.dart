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
      // As the daemon's check_path (crates/clax-core/src/publish.rs): every
      // Unicode control character (C0, DEL, C1) and U+2028/U+2029.
      expectRefused(withPath('a\u0080b'), ['control character']);
      expectRefused(withPath('a\u0085b'), ['control character']);
      expectRefused(withPath('a\u009fb'), ['control character']);
      expectRefused(withPath('a\u2028b'), ['line or paragraph separator']);
      expectRefused(withPath('a\u2029b'), ['line or paragraph separator']);
      expectRefused(withPath('a/'), ['a/', 'empty segment']);
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

      // A path is written at most once, never both written and removed,
      // and never edited when removed.
      expectRefused(
        withFiles([
          {'path': 'a.css', 'content': 'x'},
          {'path': 'a.css', 'content': 'y'},
        ]),
        ['a.css', 'twice'],
      );
      expectRefused(
        withFiles([
          {'path': 'app.js', 'content': 'x'},
        ]),
        ['app.js', 'written and removed'],
      );
      expectRefused(
        changeJson()
          ..['edits'] = [
            {'path': 'app.js', 'old_str': 'a', 'new_str': 'b'},
          ],
        ['app.js', 'edited and removed'],
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

    test('db paths follow the daemon\'s segment rule', () {
      // crates/clax-core/src/db.rs segment_ok and segments: ASCII letters,
      // digits and _ - . ~ : @ +; 1 to 200 bytes; not . or ..; at most 1000
      // bytes and 16 segments for collection/doc_id.
      Map<String, Object?> set(String coll, String doc) => changeJson()
        ..['db'] = [
          {'op': 'set', 'collection': coll, 'doc_id': doc, 'data_json': '{}'},
        ];
      void accepted(String coll, String doc) => validateResult(
        EditResult.fromJson(set(coll, doc)),
        kind: RequestKind.change,
        existingPaths: existing,
      );
      final odd15 = List.filled(15, 'c').join('/');
      final odd17 = List.filled(17, 'c').join('/');
      final s200 = 'x' * 200;
      final s199 = 'y' * 199;
      const valid = <(String, String)>[
        ('runs', 'r1'),
        ('Runs_2026-10', 'a.b~c:d@e+f'),
        ('runs', '...'),
        ('runs', 'ABCxyz0189'),
      ];
      for (final (c, d) in valid) {
        accepted(c, d);
      }
      accepted('runs', s200);
      accepted(odd15, 'd'); // 16 segments.
      accepted('$s200/$s200/$s200', s200); // 803 bytes.

      final refused = <(String, String, String)>[
        ('runs', 'my run', 'my run'),
        ('runs', '..', '..'),
        ('runs', '.', '.'),
        ('runs', 'é', 'é'),
        ('runs', 'a#b', 'a#b'),
        ('runs', 'a?b', 'a?b'),
        ('runs', 'a%20b', 'a%20b'),
        ('runs', 'a"b', 'a\\"b'),
        ('my runs', 'r1', 'my runs'),
        ('runs/../x', 'r1', '..'),
        ('runs', '${s200}z', '200 bytes'),
        (odd17, 'd', '16 segments'),
        (List.filled(5, s199).join('/'), 'd', '1000 bytes'),
      ];
      for (final (c, d, named) in refused) {
        expectRefused(set(c, d), [named]);
      }
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
    });

    test('the schema uses only keywords structured outputs accept', () {
      // Messages API structured outputs refuse minLength/maxLength,
      // minimum/maximum/multipleOf, maxItems, minItems above 1 and unlisted
      // formats, and cap union-typed parameters at 16 and optional ones at
      // 24. Pin the subset this schema uses, so an edit that adds any other
      // keyword fails here rather than as a 400 on every request.
      const allowed = {
        'type',
        'properties',
        'required',
        'additionalProperties',
        'items',
        'enum',
        'anyOf',
        'description',
      };
      var unions = 0;
      void node(Map<String, Object?> n, String at) {
        expect(allowed.containsAll(n.keys), isTrue, reason: '$at: ${n.keys}');
        expect(n['type'], isNot(isA<List<Object?>>()), reason: '$at: type');
        if (n.containsKey('anyOf')) unions++;
        if (n['type'] == 'object') {
          expect(
            (n['required'] as List).length,
            (n['properties'] as Map).length,
            reason: '$at: no optional properties',
          );
        }
        (n['properties'] as Map<String, Object?>?)?.forEach(
          (k, v) => node(v as Map<String, Object?>, '$at/properties/$k'),
        );
        if (n['items'] case final Map<String, Object?> i) node(i, '$at/items');
        if (n['anyOf'] case final List<Object?> l) {
          for (var k = 0; k < l.length; k++) {
            node(l[k] as Map<String, Object?>, '$at/anyOf[$k]');
          }
        }
      }

      node(editSchema, '#');
      expect(unions, 3, reason: 'title, data_json, capabilities_json');
      expect(unions, lessThanOrEqualTo(16));
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
