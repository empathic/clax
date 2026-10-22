import 'dart:async';

import 'package:clax_app/agent/read_consent.dart';
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

/// A fake daemon with this run's app ([app], declaring `db` but no
/// `db.read`) and another app ([other]) whose documents the agent may read
/// only once the owner allows it ([consent]).
class World {
  World() {
    app = api.seedArtifact(
      'Runs',
      {'index.html': '<p>runs</p>'},
      capabilities: {'db': <String, Object?>{}},
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

  /// The owner's answers; denies until [allowRead].
  final consent = FakeReadConsent();

  /// The owner answers [answer] to every question shown.
  void allowRead([ConsentAnswer answer = ConsentAnswer.once]) =>
      consent.answer = (_) async => answer;

  /// The document calls made on [other].
  List<FakeCall> get otherReads => [
    for (final c in api.calls)
      if (c.args['aid'] == other && c.name != 'getArtifact') c,
  ];

  /// A Toolbox for a change on [app] (a new app when [newApp]).
  Toolbox toolbox({
    bool newApp = false,
    Map<String, int?> shownDocs = const {},
    bool withConsent = true,
    StepClock? clock,
    Future<void>? cancelled,
    bool fromBundle = false,
  }) {
    clock ??= StepClock();
    final kind = newApp ? RequestKind.newApp : RequestKind.change;
    final ctx = RunContext(
      kind: kind,
      artifactId: newApp ? null : app,
      baseVersion: newApp ? 0 : 1,
      title: 'Runs',
      capabilities: newApp ? null : api.capabilitiesOf(app),
      files: const {'index.html': '<p>runs</p>'},
      shownDocs: shownDocs,
      cancelled: cancelled,
      clock: clock,
    );
    return Toolbox(
      RunState(
        ctx: ctx,
        draft: Draft(
          files: ctx.files,
          title: ctx.title,
          capabilities: ctx.capabilities,
          baseVersion: ctx.baseVersion,
        ),
        budget: RunBudget(
          newApp ? testNewCaps : testChangeCaps,
          'claude-haiku-5-5',
          clock,
        ),
        readConsent: withConsent ? consent : null,
      ),
      [...fileTools(), ...dataTools(api, fromBundle: fromBundle)],
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
          (schema['properties']! as Map).keys.toSet(),
          containsAll(schema['required']! as List),
          reason: s.name,
        );
      }
      expect(specs.every((s) => toolOrder.contains(s.name)), isTrue);
    });
  });

  group('an app from a bundle', () {
    const message =
        'This app came from someone else; it cannot read your other apps';

    test('every read tool refuses another app without asking or reading '
        'it', () async {
      final w = World()..allowRead();
      final tb = w.toolbox(fromBundle: true);
      final calls = {
        'db_collections': {'app': w.other},
        'db_list': {'collection': 'meals', 'app': w.other, 'limit': null},
        'db_get': {'path': 'meals/m1', 'app': w.other},
        'db_query': {
          'collection': 'meals',
          'app': w.other,
          'where': null,
          'order_by': null,
          'limit': null,
        },
      };
      for (final e in calls.entries) {
        expect(
          await tb.call(e.key, e.value),
          refused(contains(message)),
          reason: e.key,
        );
      }
      expect(w.consent.asks, isEmpty);
      expect([
        for (final c in w.api.calls) c.args['aid'],
      ], isNot(contains(w.other)));
    });

    test('still reads its own documents', () async {
      final w = World();
      w.api.putDoc(w.app, 'runs/r1', {'km': 5});
      final tb = w.toolbox(fromBundle: true);
      final r = await tb.call('db_get', {'path': 'runs/r1', 'app': null});
      expect((okJson(r)['doc']! as Map)['data'], {'km': 5});
      final own = await tb.call('db_get', {'path': 'runs/r1', 'app': w.app});
      expect((okJson(own)['doc']! as Map)['data'], {'km': 5});
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
      w.api
        ..putDoc(w.other, 'meals/m1', {'kcal': 500})
        ..putDoc(w.other, 'data/users/u1/n1', {'t': 'secret'});
      expect(
        await w.toolbox().call('db_collections', {'app': w.other}),
        refused('The owner did not allow reading Diet.'),
      );
      expect(w.otherReads, isEmpty);
      w.allowRead();
      final r = await w.toolbox().call('db_collections', {'app': w.other});
      expect(okJson(r), {
        'app': w.other,
        'collections': [
          {'path': 'meals', 'count': 1},
        ],
      });
    });

    test('a new app may read another app once the owner allows it', () async {
      final w = World()..allowRead();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final r = await w.toolbox(newApp: true).call('db_collections', {
        'app': w.other,
      });
      expect((okJson(r)['collections']! as List), hasLength(1));
      final ask = w.consent.asks.single;
      expect(ask.reader, isNull);
      expect(ask.source, w.other);
      expect(ask.sourceTitle, 'Diet');
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

    test('reads another app after consent, with no reader, and pins '
        'nothing', () async {
      final w = World()..allowRead();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final tb = w.toolbox();
      final r = await tb.call('db_list', {
        'collection': 'meals',
        'app': w.other,
        'limit': null,
      });
      expect(docsOf(r).single['data'], {'kcal': 500});
      expect(tb.state.draft.pins, isEmpty);
      expect(w.otherReads.single.args['reader'], isNull);
      final ask = w.consent.asks.single;
      expect(ask.reader, w.app);
      expect(ask.readerTitle, 'Runs');
      expect(ask.sourceTitle, 'Diet');
    });

    test("another app's documents without consent are refused", () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final r = await w.toolbox().call('db_list', {
        'collection': 'meals',
        'app': w.other,
        'limit': null,
      });
      expect(r, refused('The owner did not allow reading Diet.'));
      expect(w.otherReads, isEmpty);
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

    test("another app's document is read after consent", () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      w.allowRead();
      final tb = w.toolbox();
      final r = await tb.call('db_get', {'path': 'meals/m1', 'app': w.other});
      expect((okJson(r)['doc']! as Map)['data'], {'kcal': 500});
      expect(w.otherReads.single.args['reader'], isNull);
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

    test('reads another app after consent', () async {
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
      expect(w.otherReads.single.args['reader'], isNull);
    });
  });

  group('read consent', () {
    Map<String, Object?> list(String app) => {
      'collection': 'meals',
      'app': app,
      'limit': null,
    };

    test('once covers this run only', () async {
      final w = World()..allowRead();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final tb = w.toolbox();
      await tb.call('db_list', list(w.other));
      await tb.call('db_get', {'path': 'meals/m1', 'app': w.other});
      expect(w.consent.shown, 1);
      await w.toolbox().call('db_list', list(w.other));
      expect(w.consent.shown, 2);
    });

    test('always is remembered for the pair of apps', () async {
      final w = World()..allowRead(ConsentAnswer.always);
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      expect(
        docsOf(await w.toolbox().call('db_list', list(w.other))),
        hasLength(1),
      );
      final r = await w.toolbox().call('db_list', list(w.other));
      expect(docsOf(r), hasLength(1));
      expect(w.consent.shown, 1);
      expect(w.consent.asks.last.remembered, isTrue);
    });

    test('denied is not asked again this run', () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final tb = w.toolbox();
      await tb.call('db_list', list(w.other));
      final r = await tb.call('db_get', {'path': 'meals/m1', 'app': w.other});
      expect(r, refused('The owner did not allow reading Diet.'));
      expect(w.consent.shown, 1);
      expect(w.otherReads, isEmpty);
    });

    test("the app's own db.read grant is not the agent's consent", () async {
      final w = World();
      w.api
        ..capabilitiesOf(w.app)['db'] = <String, Object?>{
          'read': [w.other],
        }
        ..grantRead(w.app, w.other)
        ..putDoc(w.other, 'meals/m1', {'kcal': 500});
      final r = await w.toolbox().call('db_list', list(w.other));
      expect(r, refused('The owner did not allow reading Diet.'));
    });

    test('without a way to ask, reads of another app are refused', () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final r = await w
          .toolbox(withConsent: false)
          .call('db_list', list(w.other));
      expect(r, refused(contains("owner's consent")));
      expect(w.otherReads, isEmpty);
    });

    test('an unknown app is named, and nothing is asked', () async {
      final w = World();
      final r = await w.toolbox().call('db_list', list('zzzzzzzzzzzz'));
      expect(r, refused(contains('There is no app zzzzzzzzzzzz')));
      expect(w.consent.asks, isEmpty);
    });

    test('naming this app needs no consent', () async {
      final w = World();
      w.api.putDoc(w.app, 'meals/m1', {'kcal': 500});
      final r = await w.toolbox().call('db_list', list(w.app));
      expect(docsOf(r), hasLength(1));
      expect(w.consent.asks, isEmpty);
    });

    test('the wall clock stops while the owner is asked', () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final asked = Completer<void>();
      final answer = Completer<ConsentAnswer>();
      w.consent.answer = (_) {
        asked.complete();
        return answer.future;
      };
      final clock = StepClock();
      final tb = w.toolbox(clock: clock);
      final r = tb.call('db_list', list(w.other));
      await asked.future;
      clock.advance(const Duration(minutes: 5));
      answer.complete(ConsentAnswer.once);
      expect(docsOf(await r), hasLength(1));
      expect(tb.state.budget.timeUp, isFalse);
    });

    test('a cancelled run answers the question as denied', () async {
      final w = World();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final asked = Completer<void>();
      w.consent.answer = (_) {
        asked.complete();
        return Completer<ConsentAnswer>().future; // nobody answers
      };
      final cancel = Completer<void>();
      final clock = StepClock();
      final tb = w.toolbox(cancelled: cancel.future, clock: clock);
      final r = tb.call('db_list', list(w.other));
      await asked.future;
      expect(w.consent.asks.single.cancelled, same(cancel.future));
      cancel.complete();
      expect(await r, refused('The owner did not allow reading Diet.'));
      expect(w.otherReads, isEmpty);
      // The clock runs again, and the run does not ask again.
      clock.advance(const Duration(minutes: 5));
      expect(tb.state.budget.timeUp, isTrue);
    });

    test('a run cancelled before it asks does not ask', () async {
      final w = World()..allowRead();
      w.api.putDoc(w.other, 'meals/m1', {'kcal': 500});
      final tb = w.toolbox(cancelled: Future.value());
      final r = await tb.call('db_list', list(w.other));
      expect(r, refused('The owner did not allow reading Diet.'));
      expect(w.consent.shown, 0);
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
      w.allowRead(ConsentAnswer.always);
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

  group('private subtrees', () {
    void selfRule(Map<String, Object?> caps, {bool opened = false}) =>
        caps['db'] = <String, Object?>{
          'rules': [
            if (opened) {'path': 'prefs', 'read': 'owner', 'write': 'owner'},
            {'path': 'prefs/{self}', 'read': 'view', 'write': 'interact'},
          ],
        };

    test("a {self} rule's subtree is never read", () async {
      final w = World();
      selfRule(w.api.capabilitiesOf(w.app));
      w.api
        ..putDoc(w.app, 'prefs/u_x', {'dark': true})
        ..putDoc(w.app, 'prefs/u_x/items/a', {'n': 1})
        ..putDoc(w.app, 'runs/r1', {'km': 5});
      final tb = w.toolbox();
      expect(
        await tb.call('db_get', {'path': 'prefs/u_x', 'app': null}),
        refused(contains('private')),
      );
      expect(
        await tb.call('db_list', {
          'collection': 'prefs/u_x/items',
          'app': null,
          'limit': null,
        }),
        refused(contains('private')),
      );
      expect(
        await tb.call('db_list', {
          'collection': 'prefs',
          'app': null,
          'limit': null,
        }),
        refused(contains('private')),
      );
      expect(okJson(await tb.call('db_collections', {'app': null})), {
        'app': null,
        'collections': [
          {'path': 'runs', 'count': 1},
        ],
      });
      expect(tb.state.draft.pins, isEmpty);
    });

    test("a {self} rule's subtree is never written", () async {
      final w = World();
      selfRule(w.api.capabilitiesOf(w.app));
      final r = await w.toolbox().call('db_write', {
        'app': null,
        'ops': [
          {
            'op': 'set',
            'collection': 'prefs/u_x/items',
            'doc_id': 'a',
            'data_json': '{}',
          },
        ],
      });
      expect(r, refused(contains('private')));
    });

    test('a {self} rule the draft adds makes its subtree private', () async {
      final w = World();
      final tb = w.toolbox();
      await tb.call('set_meta', {
        'title': null,
        'capabilities_json':
            '{"db": {"rules": [{"path": "prefs/{self}", "read": "view", '
            '"write": "interact"}]}}',
      });
      final r = await tb.call('db_write', {
        'app': null,
        'ops': [
          {
            'op': 'set',
            'collection': 'prefs',
            'doc_id': 'u_x',
            'data_json': '{}',
          },
        ],
      });
      expect(r, refused(contains('private')));
    });

    test('a rule at the prefix opens its subtrees', () async {
      final w = World();
      selfRule(w.api.capabilitiesOf(w.app), opened: true);
      w.api.putDoc(w.app, 'prefs/u_x', {'dark': true});
      final r = await w.toolbox().call('db_get', {
        'path': 'prefs/u_x',
        'app': null,
      });
      expect(okJson(r)['exists'], true);
    });

    test("another app's {self} subtree is never read", () async {
      final w = World()..allowRead();
      selfRule(w.api.capabilitiesOf(w.other));
      w.api.putDoc(w.other, 'prefs/u_x', {'dark': true});
      final r = await w.toolbox().call('db_get', {
        'path': 'prefs/u_x',
        'app': w.other,
      });
      expect(r, refused(contains('private')));
    });
  });

  group('db_write bodies', () {
    Future<ToolOutput> write(
      Toolbox tb,
      String op,
      String docId,
      String data,
    ) => tb.call('db_write', {
      'app': null,
      'ops': [
        {'op': op, 'collection': 'runs', 'doc_id': docId, 'data_json': data},
      ],
    });

    String nested(int objects) {
      var s = '1';
      for (var i = 0; i < objects; i++) {
        s = '{"a": $s}';
      }
      return s;
    }

    test('a delete marker is refused in a set', () async {
      final w = World();
      expect(
        await write(w.toolbox(), 'set', 'r1', '{"old": {"__delete__": true}}'),
        refused(contains('__delete__')),
      );
    });

    test('a delete marker removes a field in an update', () async {
      final w = World();
      w.api.putDoc(w.app, 'runs/r1', {'km': 5, 'old': 1});
      final tb = w.toolbox();
      await tb.call('db_get', {'path': 'runs/r1', 'app': null});
      final r = await write(
        tb,
        'update',
        'r1',
        '{"old": {"__delete__": true}}',
      );
      expect(okJson(r)['staged'], 1);
    });

    test(
      'a delete marker in an array or as the whole body is refused',
      () async {
        final w = World();
        w.api.putDoc(w.app, 'runs/r1', {'km': 5});
        final tb = w.toolbox();
        await tb.call('db_get', {'path': 'runs/r1', 'app': null});
        expect(
          await write(tb, 'update', 'r1', '{"xs": [{"__delete__": true}]}'),
          refused(contains('__delete__')),
        );
        expect(
          await write(tb, 'update', 'r1', '{"__delete__": true}'),
          refused(contains('use delete')),
        );
        expect(tb.state.draft.stagedDb, isEmpty);
      },
    );

    test('nesting is at most 32 levels', () async {
      final w = World();
      final tb = w.toolbox();
      expect(okJson(await write(tb, 'set', 'r1', nested(31)))['staged'], 1);
      expect(
        await write(tb, 'set', 'r2', nested(32)),
        refused(contains('32 levels')),
      );
    });

    test("an update's merged body is at most 256 KiB", () async {
      final w = World();
      w.api.putDoc(w.app, 'runs/r1', {'a': 'x' * (200 * 1024)});
      final tb = w.toolbox();
      await tb.call('db_get', {'path': 'runs/r1', 'app': null});
      expect(
        await write(tb, 'update', 'r1', '{"b": "${'y' * (100 * 1024)}"}'),
        refused(allOf(contains('256 KiB'), contains('runs/r1'))),
      );
      expect(
        okJson(
          await write(tb, 'update', 'r1', '{"a": "${'y' * (100 * 1024)}"}'),
        )['staged'],
        1,
      );
    });
  });
}
