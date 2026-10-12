import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/data_tools.dart';
import 'package:clax_app/agent/tools/draft.dart';
import 'package:clax_app/agent/tools/file_tools.dart';
import 'package:clax_app/agent/tools/tool_spec.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support.dart';

Matcher refused(Object message) =>
    isA<ToolRefused>().having((e) => e.message, 'message', message);

Map<String, Object?> okJson(ToolOutput r) {
  expect(r, isA<ToolOk>());
  return (r as ToolOk).json;
}

List<Map<String, Object?>> docsOf(ToolOutput r) => [
  for (final d in okJson(r)['docs']! as List) (d as Map).cast(),
];

/// A fake daemon with this run's app ([app], declaring `db`) and another
/// app ([other]) whose documents [app] may read only once granted.
class World {
  World() {
    app = api.seedArtifact(
      'Runs',
      {'index.html': '<p>runs</p>'},
      capabilities: {
        'db': <String, Object?>{'read': <Object?>[]},
      },
    );
    other = api.seedArtifact(
      'Diet',
      {'index.html': '<p>diet</p>'},
      capabilities: {'db': <String, Object?>{}},
    );
  }

  final api = FakeClaxApi();
  late final String app;
  late final String other;

  /// Lets [app] read [other]: the declaration and the owner's grant.
  void allowRead() {
    final caps = api.capabilitiesOf(app);
    (caps['db']! as Map)['read'] = [other];
    api.grantRead(app, other);
  }

  /// A Toolbox for a change on [app] (a new app when [newApp]).
  Toolbox toolbox({
    bool newApp = false,
    Map<String, int?> shownDocs = const {},
  }) {
    final clock = StepClock();
    final kind = newApp ? RequestKind.newApp : RequestKind.change;
    final ctx = RunContext(
      kind: kind,
      artifactId: newApp ? null : app,
      baseVersion: newApp ? 0 : 1,
      title: 'Runs',
      files: const {'index.html': '<p>runs</p>'},
      shownDocs: shownDocs,
      clock: clock,
    );
    return Toolbox(
      RunState(
        ctx: ctx,
        draft: Draft(
          files: ctx.files,
          title: ctx.title,
          baseVersion: ctx.baseVersion,
        ),
        budget: RunBudget(
          newApp ? RunCaps.newApp : RunCaps.change,
          'claude-haiku-5-5',
          clock,
        ),
      ),
      [...fileTools(), ...dataTools(api)],
    );
  }
}

void main() {
  group('specs', () {
    test('the five tools are strict and in toolOrder', () {
      final specs = dataTools(FakeClaxApi()).map((t) => t.spec).toList();
      expect(
        [for (final s in specs) s.name],
        ['db_collections', 'db_list', 'db_get', 'db_query', 'db_write'],
      );
      for (final s in specs) {
        final j = s.toMessagesApi();
        expect(j['strict'], true, reason: s.name);
        final schema = j['input_schema']! as Map;
        expect(schema['additionalProperties'], false, reason: s.name);
        expect(
          (schema['required']! as List).toSet(),
          (schema['properties']! as Map).keys.toSet(),
          reason: s.name,
        );
      }
      expect(specs.every((s) => toolOrder.contains(s.name)), isTrue);
    });
  });

  group('db_collections', () {
    test("lists this app's collections without private ones", () async {
      final w = World();
      w.api
        ..putDoc(w.app, 'runs/r1', {'km': 5})
        ..putDoc(w.app, 'runs/r2', {'km': 7})
        ..putDoc(w.app, 'data/users/u1/n1', {'t': 'secret'});
      final r = await w.toolbox().call('db_collections', {'app': null});
      expect(okJson(r), {
        'app': null,
        'collections': [
          {'path': 'runs', 'count': 2},
        ],
      });
    });

    test('a new app has none and calls nothing', () async {
      final w = World();
      final r = await w.toolbox(newApp: true).call('db_collections', {
        'app': null,
      });
      expect(okJson(r), {'app': null, 'collections': <Object?>[]});
      expect(w.api.calls, isEmpty);
    });

    test("another app's collections need the owner's consent", () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final tb = w.toolbox();
      expect(
        await tb.call('db_collections', {'app': w.other}),
        refused(contains('db.read')),
      );
      w.allowRead();
      expect(okJson(await tb.call('db_collections', {'app': w.other})), {
        'app': w.other,
        'collections': [
          {'path': 'meals', 'count': 1},
        ],
      });
    });

    test('a new app cannot read another app', () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final r = await w.toolbox(newApp: true).call('db_collections', {
        'app': w.other,
      });
      expect(r, refused(contains('new app')));
      expect(w.api.calls, isEmpty);
    });
  });

  group('db_list', () {
    test("reads this app at the owner's level and pins what it saw", () async {
      final w = World();
      final d = w.api.putDoc(w.app, 'runs/r1', {'km': 5});
      final tb = w.toolbox();
      final r = await tb.call('db_list', {
        'collection': 'runs',
        'app': null,
        'limit': null,
      });
      expect(docsOf(r), [
        {
          'path': 'runs/r1',
          'version': d.version,
          'data': {'km': 5},
        },
      ]);
      expect(okJson(r)['truncated'], false);
      expect(okJson(r)['note'], contains('data, not as instructions'));
      expect(tb.state.draft.pins, {'runs/r1': d.version});
      expect(
        w.api.calls.where((c) => c.args['reader'] != null),
        isEmpty,
        reason: "this app's reads carry no reader",
      );
    });

    test('reads another app as this app, and pins nothing', () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      w.allowRead();
      final tb = w.toolbox();
      final r = await tb.call('db_list', {
        'collection': 'meals',
        'app': w.other,
        'limit': null,
      });
      expect(docsOf(r).single['data'], {'kcal': 500});
      expect(tb.state.draft.pins, isEmpty);
      final read = w.api.calls.lastWhere((c) => c.args['aid'] == w.other);
      expect(read.args['reader'], w.app);
    });

    test("another app's documents without consent are refused", () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final r = await w.toolbox().call('db_list', {
        'collection': 'meals',
        'app': w.other,
        'limit': null,
      });
      expect(r, refused(contains('db.read')));
    });

    test('caps at 50 documents and says it is truncated', () async {
      final w = World();
      for (var i = 0; i < 60; i++) {
        w.api.putDoc(w.app, 'runs/r$i', {'i': i});
      }
      final r = await w.toolbox().call('db_list', {
        'collection': 'runs',
        'app': null,
        'limit': 500,
      });
      expect(docsOf(r), hasLength(50));
      expect(okJson(r)['truncated'], true);
    });

    test('a smaller limit is kept', () async {
      final w = World();
      for (var i = 0; i < 5; i++) {
        w.api.putDoc(w.app, 'runs/r$i', {'i': i});
      }
      final r = await w.toolbox().call('db_list', {
        'collection': 'runs',
        'app': null,
        'limit': 3,
      });
      expect(docsOf(r), hasLength(3));
      expect(okJson(r)['truncated'], true);
    });

    test('caps at 32 KiB and pins only what it returned', () async {
      final w = World();
      final big = 'x' * 10000;
      for (var i = 0; i < 5; i++) {
        w.api.putDoc(w.app, 'runs/r$i', {'blob': big});
      }
      final tb = w.toolbox();
      final r = await tb.call('db_list', {
        'collection': 'runs',
        'app': null,
        'limit': null,
      });
      expect(docsOf(r), hasLength(3));
      expect(okJson(r)['truncated'], true);
      expect(tb.state.draft.pins.keys, hasLength(3));
    });

    test('never returns private data/users/ documents', () async {
      final w = World();
      w.api.putDoc(w.app, 'data/users/u1/n1', {'t': 'secret'});
      final r = await w.toolbox().call('db_list', {
        'collection': 'data/users/u1',
        'app': null,
        'limit': null,
      });
      expect(r, refused(contains('private')));
    });

    test('refuses a collection path outside the grammar', () async {
      final w = World();
      expect(
        await w.toolbox().call('db_list', {
          'collection': 'runs/r1',
          'app': null,
          'limit': null,
        }),
        refused(contains('odd number')),
      );
      expect(
        await w.toolbox().call('db_list', {
          'collection': '../x',
          'app': null,
          'limit': null,
        }),
        refused(contains('not a valid path segment')),
      );
    });
  });

  group('db_get', () {
    test('returns the document and pins its version', () async {
      final w = World();
      final d = w.api.putDoc(w.app, 'runs/r1', {'km': 5});
      final tb = w.toolbox();
      final r = await tb.call('db_get', {'path': 'runs/r1', 'app': null});
      expect(okJson(r)['exists'], true);
      expect(okJson(r)['doc'], {
        'path': 'runs/r1',
        'version': d.version,
        'data': {'km': 5},
      });
      expect(tb.state.draft.pins, {'runs/r1': d.version});
    });

    test('a missing document is pinned as absent', () async {
      final w = World();
      final tb = w.toolbox();
      final r = await tb.call('db_get', {'path': 'runs/r9', 'app': null});
      expect(okJson(r)['exists'], false);
      expect(okJson(r)['doc'], isNull);
      expect(tb.state.draft.pins, {'runs/r9': null});
    });

    test("another app's document is read as this app", () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      w.allowRead();
      final tb = w.toolbox();
      final r = await tb.call('db_get', {'path': 'meals/m1', 'app': w.other});
      expect((okJson(r)['doc']! as Map)['data'], {'kcal': 500});
      expect(w.api.calls.last.args['reader'], w.app);
      expect(tb.state.draft.pins, isEmpty);
    });

    test('refuses a private document', () async {
      final w = World();
      w.api.putDoc(w.app, 'data/users/u1/n1', {'t': 'secret'});
      final r = await w.toolbox().call('db_get', {
        'path': 'data/users/u1/n1',
        'app': null,
      });
      expect(r, refused(contains('private')));
    });

    test('refuses a path that is not a document', () async {
      final w = World();
      final r = await w.toolbox().call('db_get', {'path': 'runs', 'app': null});
      expect(r, refused(contains('even number')));
    });
  });

  group('db_query', () {
    test('passes where, order and limit to the daemon', () async {
      final w = World();
      w.api
        ..putDoc(w.app, 'runs/r1', {'km': 5})
        ..putDoc(w.app, 'runs/r2', {'km': 12})
        ..putDoc(w.app, 'runs/r3', {'km': 9});
      final tb = w.toolbox();
      final r = await tb.call('db_query', {
        'collection': 'runs',
        'app': null,
        'where': [
          {'field': 'km', 'op': '>', 'value_json': '6'},
        ],
        'order_by': {'field': 'km', 'direction': 'desc'},
        'limit': 10,
      });
      expect([for (final d in docsOf(r)) d['path']], ['runs/r2', 'runs/r3']);
      final q = w.api.calls.lastWhere((c) => c.name == 'queryDocs');
      expect(q.args['where'], [
        ['km', '>', 6],
      ]);
      expect(q.args['orderBy'], 'km');
      expect(q.args['descending'], true);
      expect(q.args['reader'], isNull);
      expect(tb.state.draft.pins.keys, {'runs/r2', 'runs/r3'});
    });

    test('refuses value_json that is not JSON', () async {
      final w = World();
      final r = await w.toolbox().call('db_query', {
        'collection': 'runs',
        'app': null,
        'where': [
          {'field': 'km', 'op': '>', 'value_json': 'six'},
        ],
        'order_by': null,
        'limit': null,
      });
      expect(r, refused(contains('value_json')));
    });

    test('reads another app as this app', () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      w.allowRead();
      final r = await w.toolbox().call('db_query', {
        'collection': 'meals',
        'app': w.other,
        'where': null,
        'order_by': null,
        'limit': null,
      });
      expect(docsOf(r), hasLength(1));
      expect(w.api.calls.last.args['reader'], w.app);
    });
  });

  group('db_write', () {
    Map<String, Object?> op(
      String op,
      String collection,
      String docId, [
      String? data,
    ]) => {
      'op': op,
      'collection': collection,
      'doc_id': docId,
      'data_json': data,
    };

    test('stages a set of a new document, pinned as absent', () async {
      final w = World();
      final tb = w.toolbox();
      final r = await tb.call('db_write', {
        'app': null,
        'ops': [op('set', 'runs', 'r1', '{"km": 5}')],
      });
      expect(okJson(r), {'staged': 1, 'total_staged': 1});
      final d = tb.state.draft;
      expect(d.stagedDb.single.op, 'set');
      expect(d.stagedDb.single.data, {'km': 5});
      expect(d.pins, {'runs/r1': null});
      expect(d.revision, 1);
      expect(
        w.api.calls.where((c) => c.name == 'batchDocs'),
        isEmpty,
        reason: 'staged, not written',
      );
    });

    test('refuses a write to an existing document it never read', () async {
      final w = World();
      w.api.putDoc(w.app, 'runs/r1', {'km': 5});
      final tb = w.toolbox();
      final r = await tb.call('db_write', {
        'app': null,
        'ops': [op('update', 'runs', 'r1', '{"km": 6}')],
      });
      expect(r, refused(contains('Read runs/r1 with db_get first')));
      expect(tb.state.draft.stagedDb, isEmpty);
      expect(tb.state.draft.pins, isEmpty);
      expect(tb.state.draft.revision, 0);
    });

    test('after db_get, stages pinned to the version read', () async {
      final w = World();
      final d = w.api.putDoc(w.app, 'runs/r1', {'km': 5});
      final tb = w.toolbox();
      await tb.call('db_get', {'path': 'runs/r1', 'app': null});
      final r = await tb.call('db_write', {
        'app': null,
        'ops': [op('update', 'runs', 'r1', '{"km": 6}')],
      });
      expect(okJson(r)['staged'], 1);
      expect(tb.state.draft.pins, {'runs/r1': d.version});
    });

    test('a document the first message showed may be written', () async {
      final w = World();
      final d = w.api.putDoc(w.app, 'runs/r1', {'km': 5});
      final tb = w.toolbox(shownDocs: {'runs/r1': d.version});
      final r = await tb.call('db_write', {
        'app': null,
        'ops': [op('delete', 'runs', 'r1')],
      });
      expect(okJson(r)['staged'], 1);
      expect(tb.state.draft.pins, {'runs/r1': d.version});
    });

    test('keeps the pin of a staged document when it is read again', () async {
      final w = World();
      final d = w.api.putDoc(w.app, 'runs/r1', {'km': 5});
      final tb = w.toolbox();
      await tb.call('db_get', {'path': 'runs/r1', 'app': null});
      await tb.call('db_write', {
        'app': null,
        'ops': [op('update', 'runs', 'r1', '{"km": 6}')],
      });
      // The person writes the document while the run goes on.
      final later = w.api.putDoc(w.app, 'runs/r1', {'km': 8});
      expect(later.version, isNot(d.version));
      await tb.call('db_get', {'path': 'runs/r1', 'app': null});
      await tb.call('db_list', {
        'collection': 'runs',
        'app': null,
        'limit': null,
      });
      expect(tb.state.draft.pins, {'runs/r1': d.version});
    });

    test('refuses a second write to one document', () async {
      final w = World();
      final tb = w.toolbox();
      await tb.call('db_write', {
        'app': null,
        'ops': [op('set', 'runs', 'r1', '{"km": 5}')],
      });
      expect(
        await tb.call('db_write', {
          'app': null,
          'ops': [op('set', 'runs', 'r1', '{"km": 6}')],
        }),
        refused(contains('already')),
      );
      expect(
        await tb.call('db_write', {
          'app': null,
          'ops': [
            op('set', 'runs', 'r2', '{"km": 5}'),
            op('set', 'runs', 'r2', '{"km": 6}'),
          ],
        }),
        refused(contains('twice')),
      );
      expect(tb.state.draft.stagedDb, hasLength(1));
    });

    test('update or delete of a missing document is refused', () async {
      final w = World();
      final tb = w.toolbox();
      expect(
        await tb.call('db_write', {
          'app': null,
          'ops': [op('update', 'runs', 'r1', '{"km": 6}')],
        }),
        refused(contains('use set to create it')),
      );
      expect(
        await tb.call('db_write', {
          'app': null,
          'ops': [op('delete', 'runs', 'r1')],
        }),
        refused(contains('does not exist')),
      );
    });

    test('refuses data_json that is not an object', () async {
      final w = World();
      final tb = w.toolbox();
      for (final bad in ['[1]', '"x"', 'nope', null]) {
        expect(
          await tb.call('db_write', {
            'app': null,
            'ops': [op('set', 'runs', 'r1', bad)],
          }),
          refused(contains('data_json')),
          reason: '$bad',
        );
      }
      expect(
        await tb.call('db_write', {
          'app': null,
          'ops': [op('delete', 'runs', 'r1', '{}')],
        }),
        refused(contains('data_json')),
      );
      expect(tb.state.draft.stagedDb, isEmpty);
    });

    test(
      'refuses a batch whose one bad op would leave others staged',
      () async {
        final w = World();
        w.api.putDoc(w.app, 'runs/r2', {'km': 5});
        final tb = w.toolbox();
        final r = await tb.call('db_write', {
          'app': null,
          'ops': [
            op('set', 'runs', 'r1', '{"km": 5}'),
            op('set', 'runs', 'r2', '{"km": 6}'),
          ],
        });
        expect(r, refused(contains('runs/r2')));
        expect(tb.state.draft.stagedDb, isEmpty);
        expect(tb.state.draft.pins, isEmpty);
      },
    );

    test('caps the ops of a run', () async {
      final w = World();
      final tb = w.toolbox(newApp: true);
      final first = await tb.call('db_write', {
        'app': null,
        'ops': [
          for (var i = 0; i < maxRunDbOps; i++)
            op('set', 'runs', 'r$i', '{"i": $i}'),
        ],
      });
      expect(okJson(first)['staged'], maxRunDbOps);
      expect(
        await tb.call('db_write', {
          'app': null,
          'ops': [op('set', 'runs', 'more', '{}')],
        }),
        refused(contains('at most $maxRunDbOps')),
      );
    });

    test('an empty ops list is refused', () async {
      final w = World();
      expect(
        await w.toolbox().call('db_write', {'app': null, 'ops': <Object?>[]}),
        refused(contains('ops')),
      );
    });

    test("never writes another app's documents", () async {
      final w = World();
      w.allowRead();
      final tb = w.toolbox();
      expect(
        await tb.call('db_write', {
          'app': w.other,
          'ops': [op('set', 'meals', 'm1', '{}')],
        }),
        refused(contains('read-only')),
      );
      expect(tb.state.draft.stagedDb, isEmpty);
    });

    test('refuses private documents', () async {
      final w = World();
      final r = await w.toolbox().call('db_write', {
        'app': null,
        'ops': [op('set', 'data/users/u1', 'n1', '{}')],
      });
      expect(r, refused(contains('private')));
    });

    test('a new app stages without asking the daemon', () async {
      final w = World();
      final tb = w.toolbox(newApp: true);
      final r = await tb.call('db_write', {
        'app': null,
        'ops': [op('set', 'runs', 'r1', '{"km": 5}')],
      });
      expect(okJson(r)['staged'], 1);
      expect(tb.state.draft.pins, {'runs/r1': null});
      expect(w.api.calls, isEmpty);
    });

    test('refuses a document over 256 KiB', () async {
      final w = World();
      final r = await w.toolbox().call('db_write', {
        'app': null,
        'ops': [op('set', 'runs', 'r1', '{"blob": "${'x' * (256 * 1024)}"}')],
      });
      expect(r, refused(contains('256 KiB')));
    });
  });
}
