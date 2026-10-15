import 'dart:io';

import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/doc_snapshot.dart';
import 'package:clax_app/bundle/updater.dart';
import 'package:clax_app/clax/api.dart';
import 'package:flutter_test/flutter_test.dart';

import 'services_support.dart';

void main() {
  late Directory tmp;
  late Install me;
  late String aid;

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('bs-update');
    me = Install(tmp);
    await me.ready();
    final v1 = await makeBundle(
      lineage: lineageA,
      files: {'index.html': '<p>v1</p>', 'old.js': 'gone soon'},
    );
    aid = await me.installer.install(await me.installer.open(v1.bytes));
    me.api.putDoc(aid, 'plants/p1', {'name': 'fern'});
    me.api.putDoc(aid, 'plants/p2', {'name': 'moss'});
    me.api.calls.clear();
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  Future<Bundle> v2({bool migrate = false, bool tests = false}) async =>
      BundleReader.read(
        (await makeBundle(
          lineage: lineageA,
          version: 2,
          capabilities: {
            'db': {},
            'net': {
              'hosts': ['api.example.com'],
            },
          },
          files: {
            'index.html': '<p>v2</p>',
            if (migrate) 'migrate.js': 'export default async () => {}',
            if (tests) 'tests/a.test.js': 'test("a", () => {})',
          },
        )).bytes,
      );

  Future<Map<String, Map<String, Object?>>> docs() async => {
    await for (final d in ownerDocs(me.api, aid)) d.path: d.data,
  };

  test('addedCapabilities lists new capabilities and hosts', () {
    final d = addedCapabilities(
      {
        'db': {},
        'net': {
          'hosts': ['a.com'],
        },
      },
      {
        'db': {},
        'notify': {},
        'net': {
          'hosts': ['a.com', 'b.com'],
        },
      },
    );
    expect(d.capabilities, ['notify']);
    expect(d.hosts, ['b.com']);
  });

  test('a passing update publishes every file, null for removed ones, '
      'deletes the draft and moves the lineage', () async {
    final b = await v2(migrate: true, tests: true);
    final steps = <UpdateStep>[];
    final out = await me.updater.update(aid, b, onStep: steps.add);
    expect(out, isA<UpdateLanded>());
    expect(steps, [
      UpdateStep.data,
      UpdateStep.migrate,
      UpdateStep.page,
      UpdateStep.tests,
      UpdateStep.publish,
      UpdateStep.liveMigrate,
    ]);
    final draft = me.api.calls.firstWhere((c) => c.name == 'createDraft');
    expect(draft.args['of'], aid);
    expect(draft.args['inheritNet'], isFalse, reason: 'a check runs offline');
    expect(me.api.callNames, contains('deleteDraft'));
    final pub = me.api.calls.firstWhere((c) => c.name == 'publish');
    final r = pub.args['request']! as PublishRequest;
    expect(r.ifVersion, 1);
    expect(r.entries.containsKey('old.js'), isTrue);
    expect(r.entries['old.js'], isNull);
    expect(r.label, 'Update v2');
    expect(me.inspector.migrationCalls, hasLength(2));
    expect(me.inspector.migrationCalls.first.from, 1);
    expect(me.inspector.migrationCalls.first.to, 2);
    expect(me.inspector.migrationCalls.last.url.pathSegments, [
      'a',
      aid,
    ], reason: 'the second run is on the live app');
    final live = await me.api.getArtifact(aid);
    expect(live.current!.contentSha256, b.manifest.contentSha256);
    final e = (await me.store.entry(aid))!;
    expect(e.version, 2);
    expect(e.released.last, b.manifest.contentSha256);
    expect(await me.store.keptBundle(aid), b.bytes);
    expect(me.snapshots.has(aid), isFalse);
  });

  Future<void> expectUnchanged(UpdateOutcome out) async {
    expect(me.api.callNames, isNot(contains('publish')));
    expect(me.api.callNames, contains('deleteDraft'));
    expect((await me.store.entry(aid))!.version, 1);
    expect((await me.api.getArtifact(aid)).artifact.currentVersion, 1);
  }

  test('a migration error fails the check', () async {
    me.inspector.answerMigration(const MigrationRun.failed('boom'));
    final out = await me.updater.update(aid, await v2(migrate: true));
    expect(out, isA<UpdateCheckFailed>());
    expect((out as UpdateCheckFailed).step, UpdateStep.migrate);
    expect(out.details, ['boom']);
    await expectUnchanged(out);
  });

  test('an uncaught page error fails the check', () async {
    me.inspector.answer(
      me.inspector.clean(errors: [(kind: 'error', text: 'x is undefined')]),
    );
    final out = await me.updater.update(aid, await v2());
    expect((out as UpdateCheckFailed).step, UpdateStep.page);
    expect(out.details, ['x is undefined']);
    await expectUnchanged(out);
  });

  test('a failed test fails the check', () async {
    me.inspector.answerTests(
      const TestRun(
        passed: 0,
        failed: [(name: 'a', error: 'expected 1')],
        skipped: 0,
      ),
    );
    final out = await me.updater.update(aid, await v2(tests: true));
    expect((out as UpdateCheckFailed).step, UpdateStep.tests);
    expect(out.details, ['a: expected 1']);
    await expectUnchanged(out);
  });

  test('tests that did not run fail the check', () async {
    me.inspector.failTests(const InspectorError('did not load'));
    final out = await me.updater.update(aid, await v2(tests: true));
    expect((out as UpdateCheckFailed).step, UpdateStep.tests);
    await expectUnchanged(out);
  });

  test('a 409 on publish: changed during the check', () async {
    me.api.failNext(
      'publish',
      const ClaxApiError(
        status: 409,
        code: 'conflict',
        message: 'stale',
        body: {'current': 2},
      ),
    );
    final out = await me.updater.update(aid, await v2());
    expect(out, isA<UpdateRefused>());
    expect(
      (out as UpdateRefused).message,
      'Plant log changed while the update was checked. Try again.',
    );
    expect((await me.store.entry(aid))!.version, 1);
  });

  test('refused while the agent works on the app', () async {
    me.busy.add(aid);
    final out = await me.updater.update(aid, await v2());
    expect(out, isA<UpdateRefused>());
    expect(me.api.callNames, isNot(contains('createDraft')));
  });

  test(
    'a failed live migration restores the documents and the version',
    () async {
      final inspector = me.inspector;
      // The draft run passes; the live one writes, then fails.
      final b = await v2(migrate: true);
      final updater = Updater(
        api: me.api,
        store: me.store,
        inspector: _Writing(inspector, () async {
          me.api.putDoc(aid, 'plants/p1', {'name': 'FERN', 'v': 2});
          me.api.putDoc(aid, 'plants/p3', {'name': 'new'});
          final p2 = await me.api.getDoc(aid, 'plants/p2');
          await me.api.batchDocs(aid, [
            DocWrite(
              op: 'delete',
              collection: 'plants',
              docId: 'p2',
              ifVersion: p2!.version,
            ),
          ]);
        }),
        baseUrl: () => Uri.parse('http://127.0.0.1:9/'),
        snapshots: me.snapshots,
      );
      final out = await updater.update(aid, b);
      expect(out, isA<UpdateRolledBack>());
      expect((out as UpdateRolledBack).backOn, 1);
      expect(out.restored, isTrue);
      expect(await docs(), {
        'plants/p1': {'name': 'fern'},
        'plants/p2': {'name': 'moss'},
      });
      final live = await me.api.getArtifact(aid);
      expect(
        live.artifact.currentVersion,
        3,
        reason: 'v2 published, v1 restored',
      );
      expect(await me.api.readFile(aid, 3, 'index.html'), '<p>v1</p>');
      expect((await me.store.entry(aid))!.version, 1);
      expect(me.snapshots.has(aid), isFalse);
    },
  );

  test('a snapshot left at start is restored with the version, then '
      'deleted', () async {
    final b = await v2(migrate: true);
    await me.snapshots.take(
      aid,
      SnapshotMeta(
        base: 1,
        contentSha256: b.manifest.contentSha256,
        title: 'Plant log',
        backOn: 1,
      ),
    );
    // The update was published and its migration wrote, then Clax died.
    await me.api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        entries: {
          for (final MapEntry(:key, :value) in b.files.entries)
            key: FileEntry.bytes(value, type: b.manifest.files[key]!.type),
          'old.js': null,
        },
      ),
    );
    me.snapshots.writeMeta(aid, me.snapshots.meta(aid)!.copyWith(published: 2));
    me.api.putDoc(aid, 'plants/p9', {'name': 'stray'});

    final got = await me.updater.recoverInterrupted();
    expect(got.single.aid, aid);
    expect(got.single.backOn, 1);
    expect(await docs(), {
      'plants/p1': {'name': 'fern'},
      'plants/p2': {'name': 'moss'},
    });
    final live = await me.api.getArtifact(aid);
    expect(
      await me.api.readFile(aid, live.artifact.currentVersion, 'old.js'),
      'gone soon',
    );
    expect(me.snapshots.has(aid), isFalse);
    expect(await me.updater.recoverInterrupted(), isEmpty);
  });

  test('the snapshot covers every collection, beyond one page', () async {
    for (var i = 0; i < 250; i++) {
      me.api.putDoc(aid, 'logs/l$i', {'i': i});
    }
    me.api.putDoc(aid, 'logs/l0/notes/n1', {'sub': true});
    final n = await me.snapshots.take(
      aid,
      const SnapshotMeta(base: 1, contentSha256: 'x', title: 't', backOn: 1),
    );
    expect(n, 253);
    expect(
      (await me.snapshots.read(aid)).map((d) => d.path).toSet(),
      (await docs()).keys.toSet(),
    );
  });
}

/// An [Inspector] whose second migration run calls [write] and then
/// fails, as a `migrate.js` that breaks part way on the live app.
class _Writing implements Inspector {
  _Writing(this.inner, this.write);

  final Inspector inner;
  final Future<void> Function() write;
  var _runs = 0;

  @override
  Future<Inspection> inspect(
    Uri url, {
    Duration settle = const Duration(seconds: 3),
    int maxBytes = 1 << 20,
    Future<void>? cancelled,
  }) => inner.inspect(url, settle: settle, maxBytes: maxBytes);

  @override
  Future<TestRun> runTests(
    Uri url, {
    required List<String> files,
    Duration perTest = const Duration(seconds: 5),
    Duration total = const Duration(seconds: 30),
    Future<void>? cancelled,
  }) => inner.runTests(url, files: files);

  @override
  Future<void> restoreWrites(
    Uri url,
    List<MigrationWrite> writes, {
    Duration total = const Duration(seconds: 60),
    Future<void>? cancelled,
  }) async {}

  @override
  Future<MigrationRun> runMigration(
    Uri url, {
    String file = 'migrate.js',
    required int from,
    required int to,
    Duration total = const Duration(seconds: 30),
    Future<void>? cancelled,
    Future<void> Function(MigrationWrite write)? onWrite,
  }) async {
    if (++_runs == 1) return const MigrationRun.passed();
    await write();
    return const MigrationRun.failed('TypeError: x.map is not a function');
  }
}
