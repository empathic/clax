import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/doc_snapshot.dart';
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/updater.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_inspector.dart';
import 'services_support.dart';

/// The rollback, snapshot and race rules of an update (bundles spec §8.2
/// step 7 and §8.4).
void main() {
  late Directory tmp;
  late Install me;
  late String aid;

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('bs-safety');
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

  Future<Bundle> v2({int version = 2}) async => BundleReader.read(
    (await makeBundle(
      lineage: lineageA,
      version: version,
      title: 'Plant diary',
      capabilities: {
        'db': {},
        'net': {
          'hosts': ['api.example.com'],
        },
      },
      files: {
        'index.html': '<p>v$version</p>',
        'migrate.js': 'export default async () => {}',
      },
    )).bytes,
  );

  Future<Map<String, Object?>> docs() async => {
    await for (final d in ownerDocs(me.api, aid)) d.path: d.data,
  };

  const before = {
    'plants/p1': {'name': 'fern'},
    'plants/p2': {'name': 'moss'},
  };

  Updater updaterWith(Inspector inspector, {AppHold? hold}) => Updater(
    api: me.api,
    store: me.store,
    inspector: inspector,
    baseUrl: () => Uri.parse('http://127.0.0.1:9/'),
    snapshots: me.snapshots,
    hold: hold,
  );

  /// A live migration that writes, then fails.
  Inspector breaking() => _Hooked(
    me.inspector,
    onMigrate: (run) async {
      if (run == 1) return null;
      me.api.putDoc(aid, 'plants/p1', {'name': 'FERN'});
      me.api.putDoc(aid, 'plants/p3', {'name': 'new'});
      return 'TypeError: boom';
    },
  );

  test('a failed live migration puts back v1\'s files, title, icon and '
      'capabilities, so the app reads as unmodified', () async {
    final out = await updaterWith(breaking()).update(aid, await v2());
    expect(out, isA<UpdateRolledBack>());
    expect((out as UpdateRolledBack).restored, isTrue);
    final d = await me.api.getArtifact(aid);
    expect(d.artifact.title, 'Plant log');
    expect(d.artifact.icon, 'leaf');
    expect(d.artifact.capabilities, {'db': {}});
    expect(d.current!.contentSha256, d.versions.first.contentSha256);
    expect(await docs(), before);
    final e = (await me.store.entry(aid))!;
    final live = (await liveRelease(me.api, aid))!;
    expect(
      e.isModified(
        contentSha256: live.contentSha256,
        capabilities: live.capabilities,
      ),
      isFalse,
    );
    expect(me.snapshots.has(aid), isFalse);
  });

  test(
    'a failed rollback keeps the snapshot and refuses further updates',
    () async {
      final updater = updaterWith(breaking());
      final b = await v2();
      final failing = _Hooked(
        me.inspector,
        onMigrate: (run) async {
          if (run == 1) return null;
          me.api.putDoc(aid, 'plants/p3', {'name': 'new'});
          // The restore's first write fails (not a conflict).
          me.api.failNext(
            'batchDocs',
            const ClaxApiError(status: 500, code: 'internal', message: 'x'),
          );
          return 'boom';
        },
      );
      final out = await updaterWith(failing).update(aid, b);
      expect((out as UpdateRolledBack).restored, isFalse);
      expect(me.snapshots.has(aid), isTrue);
      final kept = me.snapshots.file(aid).readAsStringSync();
      expect(me.snapshots.meta(aid)!.failed, isTrue);

      final again = await updater.update(aid, b);
      expect(again, isA<UpdateRefused>());
      expect(me.snapshots.file(aid).readAsStringSync(), kept);
      await expectLater(
        me.snapshots.take(
          aid,
          const SnapshotMeta(
            base: 1,
            contentSha256: 'x',
            title: 't',
            backOn: 1,
          ),
        ),
        throwsStateError,
      );

      // At start: kept for the person, never restored over later writes.
      me.api.putDoc(aid, 'plants/p4', {'name': 'written later'});
      final got = await updater.recoverInterrupted();
      expect(got.single.restored, isFalse);
      expect((await docs()).containsKey('plants/p4'), isTrue);

      // Restore: the documents return to before the update.
      expect(await updater.restoreKept(aid), isTrue);
      expect(await docs(), before);
      expect(me.snapshots.has(aid), isFalse);
    },
  );

  Future<void> interrupt(Bundle b) async {
    final live = await me.api.getArtifact(aid);
    await me.snapshots.take(
      aid,
      SnapshotMeta(
        base: 1,
        contentSha256: b.manifest.contentSha256,
        title: live.artifact.title,
        icon: live.artifact.icon,
        capabilities: live.artifact.capabilities,
        backOn: 1,
      ),
    );
    final r = await me.api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        title: b.manifest.title,
        capabilities: b.manifest.capabilities,
        entries: {
          for (final MapEntry(:key, :value) in b.files.entries)
            key: FileEntry.bytes(value, type: b.manifest.files[key]!.type),
          'old.js': null,
        },
      ),
    );
    me.snapshots.writeMeta(
      aid,
      me.snapshots.meta(aid)!.copyWith(published: r.version.n),
    );
    me.api.putDoc(aid, 'plants/p1', {'name': 'migrated'});
  }

  test('an interrupted update whose version is still live is put back at '
      'start, declaration included', () async {
    await interrupt(await v2());
    final got = await me.updater.recoverInterrupted();
    expect(got.single.restored, isTrue);
    expect(await docs(), before);
    final d = await me.api.getArtifact(aid);
    expect(d.artifact.title, 'Plant log');
    expect(d.artifact.capabilities, {'db': {}});
  });

  test('an interrupted update the app moved on from is kept for the '
      'person; Discard keeps the documents', () async {
    await interrupt(await v2());
    me.api.seedVersion(aid, {'index.html': '<p>later</p>'});
    me.api.putDoc(aid, 'plants/p5', {'name': 'later'});
    final got = await me.updater.recoverInterrupted();
    expect(got.single.restored, isFalse);
    expect((await docs()).containsKey('plants/p5'), isTrue);
    expect(me.snapshots.has(aid), isTrue);
    await me.updater.discardKept(aid);
    expect(me.snapshots.has(aid), isFalse);
    expect((await docs()).containsKey('plants/p5'), isTrue);
  });

  test('a snapshot whose count does not match is never restored', () async {
    await me.snapshots.take(
      aid,
      const SnapshotMeta(base: 1, contentSha256: 'x', title: 't', backOn: 1),
    );
    expect(me.snapshots.meta(aid)!.count, 2);
    me.snapshots.file(aid).writeAsStringSync('');
    me.api.calls.clear();
    await expectLater(me.snapshots.restore(aid), throwsA(isA<SnapshotError>()));
    expect(me.api.callNames, isNot(contains('batchDocs')));
  });

  test('restore deletes what the snapshot lacks before it sets', () async {
    await me.snapshots.take(
      aid,
      const SnapshotMeta(base: 1, contentSha256: 'x', title: 't', backOn: 1),
    );
    me.api.putDoc(aid, 'plants/p3', {'name': 'new'});
    me.api.calls.clear();
    await me.snapshots.restore(aid);
    final ops = [
      for (final c in me.api.calls)
        if (c.name == 'batchDocs')
          for (final w in c.args['writes']! as List<DocWrite>) w.op,
    ];
    expect(ops, ['delete', 'set', 'set']);
  });

  test('a bundle no longer above the lineage\'s version is refused right '
      'before the publish', () async {
    final hooked = _Hooked(
      me.inspector,
      onInspect: () =>
          me.store.update(aid, (e) async => e!.copyWith(version: 5)),
    );
    final b = await BundleReader.read(
      (await makeBundle(lineage: lineageA, version: 2)).bytes,
    );
    final out = await updaterWith(hooked).update(aid, b);
    expect(out, isA<UpdateRefused>());
    expect(me.api.callNames, isNot(contains('publish')));
  });

  test('the update holds the agent off and marks the app updating for its '
      'whole run, and releases both', () async {
    final held = <String>[];
    var holding = false;
    final seen = <bool>[];
    late Updater updater;
    updater = updaterWith(
      _Hooked(
        me.inspector,
        onInspect: () async =>
            seen.add(holding && updater.updating.value.contains(aid)),
      ),
      hold: <T>(String a, Future<T> Function() body) async {
        held.add(a);
        holding = true;
        try {
          return await body();
        } finally {
          holding = false;
        }
      },
    );
    final out = await updater.update(aid, await v2());
    expect(out, isA<UpdateLanded>());
    expect(held, [aid]);
    expect(seen, [true]);
    expect(updater.updating.value, isEmpty);
  });

  test('an inspector that throws something else: the draft is deleted and '
      'update returns', () async {
    final out = await updaterWith(
      _Hooked(me.inspector, onInspect: () => throw StateError('gone')),
    ).update(aid, await v2());
    expect(out, isA<UpdateRefused>());
    expect(me.api.callNames, contains('deleteDraft'));
    expect(me.updater.updating.value, isEmpty);
  });

  test('a release published before a crash is written to the lineage at '
      'start', () async {
    final b = await v2();
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
    final dir = Directory('${me.store.support.path}/updates')
      ..createSync(recursive: true);
    File('${dir.path}/$aid.clax').writeAsBytesSync(b.bytes);
    File('${dir.path}/$aid.json').writeAsStringSync(
      jsonEncode({
        'content_sha256': b.manifest.contentSha256,
        'version': 2,
        'title': b.manifest.title,
        'capabilities': b.manifest.capabilities,
        'name': 'Sam',
      }),
    );
    expect(await me.updater.recoverInterrupted(), isEmpty);
    final e = (await me.store.entry(aid))!;
    expect(e.version, 2);
    expect(e.released.last, b.manifest.contentSha256);
    expect(await me.store.keptBundle(aid), b.bytes);
    expect(dir.listSync(), isEmpty);
  });

  test('a lineage write that throws is an outcome, not an exception', () async {
    final b = await v2();
    // The kept bundle's directory is a file: keepBundle throws.
    Directory('${me.store.support.path}/bundles').deleteSync(recursive: true);
    File('${me.store.support.path}/bundles').writeAsStringSync('x');
    final out = await me.updater.update(aid, b);
    expect(out, isA<UpdateRefused>());
  });

  test('install records the lineage as from a bundle before the app is '
      'filled, and only for an install decision', () async {
    // Another install: its fake daemon numbers apps from 1 too.
    final store = LineageStore(Directory('${tmp.path}/other'));
    final api = _Seeing(fromBundleOf(store));
    await api.registerAppSession(installId: 'i', pid: 1);
    final inst = Installer(api: api, store: store, key: me.key);
    final w = await makeBundle(
      lineage: 'l_bbbbbbbbbbbbbbbbbbbbbbbbbb',
      seed: [
        const SeedDoc('plants/p1', {'n': 1}),
      ],
    );
    final made = await inst.install(await inst.open(w.bytes));
    expect(api.seen, [true], reason: 'from a bundle while it is seeded');
    expect((await store.entry(made))!.role, LineageRole.installed);
    expect(installMarker(store, w.manifest.lineage).existsSync(), isFalse);

    final v2b = await makeBundle(lineage: lineageA, version: 2);
    final up = await me.installer.open(v2b.bytes);
    await expectLater(me.installer.install(up), throwsA(isA<BundleFailure>()));
  });
}

/// A [FakeClaxApi] that asks [fromBundle] about each app it seeds.
class _Seeing extends FakeClaxApi {
  _Seeing(this.fromBundle);

  final Future<bool> Function(String aid) fromBundle;
  final seen = <bool>[];

  @override
  Future<void> batchDocs(String aid, List<DocWrite> writes) async {
    seen.add(await fromBundle(aid));
    return super.batchDocs(aid, writes);
  }
}

/// An [Inspector] over [inner] that runs [onInspect] before each page
/// load, and answers each migration run (counted from 1) with
/// [onMigrate]'s error, or a pass for null.
class _Hooked implements Inspector {
  _Hooked(this.inner, {this.onInspect, this.onMigrate});

  final FakeInspector inner;
  final Future<void> Function()? onInspect;
  final Future<String?> Function(int run)? onMigrate;
  var _runs = 0;

  @override
  Future<Inspection> inspect(
    Uri url, {
    Duration settle = const Duration(seconds: 3),
    int maxBytes = 1 << 20,
    Future<void>? cancelled,
  }) async {
    await onInspect?.call();
    return inner.inspect(url);
  }

  @override
  Future<TestRun> runTests(
    Uri url, {
    required List<String> files,
    Duration perTest = const Duration(seconds: 5),
    Duration total = const Duration(seconds: 30),
    Future<void>? cancelled,
  }) => inner.runTests(url, files: files);

  @override
  Future<MigrationRun> runMigration(
    Uri url, {
    String file = 'migrate.js',
    required int from,
    required int to,
    Duration total = const Duration(seconds: 30),
    Future<void>? cancelled,
  }) async {
    final f = onMigrate;
    if (f == null) return const MigrationRun.passed();
    final error = await f(++_runs);
    return error == null
        ? const MigrationRun.passed()
        : MigrationRun.failed(error);
  }
}
