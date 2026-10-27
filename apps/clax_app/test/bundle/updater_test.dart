import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/author_trust.dart';
import 'package:clax_app/agent/icon_svg.dart';
import 'package:clax_app/agent/inspector.dart';
import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/doc_snapshot.dart';
import 'package:clax_app/bundle/installer.dart' show PublishedFiles;
import 'package:clax_app/bundle/updater.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/remote/merge.dart' show Merged;
import 'package:clax_app/remote/models.dart';
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
    aid = (await me.installer.install(await me.installer.open(v1.bytes))).aid;
    me.api.putDoc(aid, 'plants/p1', {'name': 'fern'});
    me.api.putDoc(aid, 'plants/p2', {'name': 'moss'});
    me.api.calls.clear();
    me.api.approved.clear();
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

  test('an accepted update approves its version', () async {
    final out = await me.updater.update(
      aid,
      await v2(),
      confirm: () async => true,
    );
    expect(out, isA<UpdateLanded>());
    expect(me.api.approved, [(aid, 2, FakeClaxApi.pendingSha256(aid, 2))]);
  });

  test('an automatic update approves nothing', () async {
    final out = await me.updater.update(aid, await v2());
    expect(out, isA<UpdateLanded>());
    expect(me.api.approved, isEmpty);
  });

  test('a refused approval leaves the accepted update landed', () async {
    me.api.failNext(
      'approveCapabilities',
      ClaxApiError(status: 409, code: 'stale_version', message: 'moved on'),
    );
    final out = await me.updater.update(
      aid,
      await v2(),
      confirm: () async => true,
    );
    expect(out, isA<UpdateLanded>());
    expect(me.api.approved, isEmpty);
    expect((await me.api.getArtifact(aid)).current!.n, 2);
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

  test('an update whose icon the icon rules refuse publishes it cleaned, '
      'or leaves it out, and says so', () async {
    const fixable =
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 9">'
        '<title>A & B</title><rect width="9" height="9"/></svg>';
    final b = await BundleReader.read(
      (await makeBundle(
        lineage: lineageA,
        version: 2,
        files: {'index.html': '<p>v2</p>', 'icon.svg': fixable},
      )).bytes,
    );
    final out = await me.updater.update(aid, b);
    expect(out, isA<UpdateLanded>());
    expect(
      PublishedFiles.of(b).iconChange,
      'Its icon broke Clax’s icon rules, so Clax redrew it.',
    );
    final live = await me.api.getArtifact(aid);
    expect(await me.api.readFile(aid, 2, 'icon.svg'), cleanIcon(fixable));
    expect(live.current!.note, endsWith('; its icon was redrawn (icon rule '
        'not_xml)'));
    final hash = live.current!.contentSha256!;
    expect(hash, isNot(b.manifest.contentSha256));
    final e = (await me.store.entry(aid))!;
    expect(e.released.last, b.manifest.contentSha256);
    expect(e.released, contains(hash));
    expect(
      e.isModified(contentSha256: hash, capabilities: e.capabilities),
      isFalse,
    );

    const hopeless =
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 8"/>';
    final b3 = await BundleReader.read(
      (await makeBundle(
        lineage: lineageA,
        version: 3,
        files: {'index.html': '<p>v3</p>', 'icon.svg': hopeless},
      )).bytes,
    );
    final out3 = await me.updater.update(aid, b3);
    expect(out3, isA<UpdateLanded>());
    expect(
      PublishedFiles.of(b3).iconChange,
      'Its icon broke Clax’s icon rules, so Clax left it out.',
    );
    final v3 = await me.api.getArtifact(aid);
    expect(v3.current!.files.keys, ['index.html']);
  });

  test('an update whose icon is not UTF-8 publishes it re-read as Latin-1 '
      'when that passes, and says so', () async {
    final b = await BundleReader.read(
      (await makeBundle(
        lineage: lineageA,
        version: 2,
        files: {'index.html': '<p>v2</p>'},
        rawFiles: {
          'icon.svg': latin1.encode(
            '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 9">'
            '<title>Caf\u00e9</title><rect width="9" height="9"/></svg>',
          ),
        },
      )).bytes,
    );
    final out = await me.updater.update(aid, b);
    expect(out, isA<UpdateLanded>());
    expect(
      PublishedFiles.of(b).iconChange,
      'Its icon broke Clax’s icon rules, so Clax redrew it.',
    );
    expect((await me.api.getArtifact(aid)).current!.note, endsWith('; its '
        'icon was redrawn (icon rule not_xml)'));
    expect(
      await me.api.readFile(aid, 2, 'icon.svg'),
      contains('<title>Caf\u00e9</title>'),
    );
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

  test('a cancel during the check stops it: the draft goes, nothing is '
      'published', () async {
    me.inspector.hangTests();
    final cancel = Completer<void>();
    final out = await me.updater.update(
      aid,
      await v2(tests: true),
      cancelled: cancel.future,
      onStep: (s) {
        if (s == UpdateStep.tests) cancel.complete();
      },
    );
    expect(out, isA<UpdateCancelled>());
    expect(me.inspector.testCalls.single.cancelled, isNotNull);
    await expectUnchanged(out);
    expect(me.snapshots.has(aid), isFalse);
  });

  test('a cancel before the check starts makes no draft', () async {
    final out = await me.updater.update(
      aid,
      await v2(),
      cancelled: Future<void>.value(),
    );
    expect(out, isA<UpdateCancelled>());
    expect(me.api.callNames, isNot(contains('createDraft')));
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

  group('linked', () {
    const remote = 'https://clax.example';
    const by = PublishedBy(name: 'Sam');
    BundleOrigin from(String raid) => BundleOrigin(
      remoteOrigin: remote,
      remoteArtifactId: raid,
      remoteVersion: 2,
      head: RemoteHead(
        artifactId: raid,
        url: '$remote/a/$raid',
        title: 'Plant log',
        version: 2,
        contentSha256: 'sha256:${'0' * 64}',
        publishedBy: by,
        owner: by,
        comments: false,
      ),
    );

    // v2 of the lineage, signed by another key than v1's (an editor's).
    Future<Bundle> v2ByEditor() async => BundleReader.read(
      (await makeBundle(
        lineage: lineageA,
        version: 2,
        files: {'index.html': '<p>v2 by an editor</p>'},
        seedByte: 5,
        name: 'Ed',
      )).bytes,
    );

    Future<void> linkTo(String raid) => me.store.update(
      aid,
      (e) async => e!.copyWith(
        link: () => AppLink(
          remoteOrigin: remote,
          remoteId: 'r1',
          remoteArtifactId: raid,
          url: '$remote/a/$raid',
          baseRemoteVersion: 1,
          role: AppLink.recipient,
        ),
      ),
    );

    test('linked_update_with_new_key_lands', () async {
      await linkTo('ra1');
      final b = await v2ByEditor();
      final key = (await me.store.entry(aid))!.key;
      expect(b.manifest.author.key, isNot(key));
      final out = await me.updater.update(aid, b, origin: from('ra1'));
      expect(out, isA<UpdateLanded>());
      final e = (await me.store.entry(aid))!;
      expect(e.version, 2);
      expect(e.released.last, b.manifest.contentSha256);
      expect(e.link?.remoteArtifactId, 'ra1', reason: 'the link stays');
      final live = await me.api.getArtifact(aid);
      expect(live.current!.contentSha256, b.manifest.contentSha256);
    });

    test('an update from the linked copy moves the core’s link first; '
        'refused, neither base moves and the outcome says why', () async {
      await linkTo('ra1');
      final moves = <(String, String, int)>[];
      me.updater.relink = (rid, aid, app, n) async {
        moves.add((rid, app.artifactId, n));
        throw const ClaxApiError(
          status: 403,
          code: 'not_permitted',
          message: 'this account may not link it',
        );
      };
      final out = await me.updater.update(
        aid,
        await v2ByEditor(),
        origin: from('ra1'),
      );
      expect(moves, [('r1', 'ra1', 2)]);
      expect(out, isA<UpdateLanded>());
      expect((out as UpdateLanded).notLinked, 'this account may not link it');
      final e = (await me.store.entry(aid))!;
      expect(e.version, 2, reason: 'the update itself is live');
      expect(e.link!.baseRemoteVersion, 1, reason: 'the base stays');
    });

    test('an editor’s update is who published the live version, and the '
        'next change asks about them once', () async {
      await linkTo('ra1');
      final mine = (await me.store.entry(aid))!.key;
      final b = await v2ByEditor();
      final out = await me.updater.update(aid, b, origin: from('ra1'));
      expect(out, isA<UpdateLanded>());
      final from_ = (await me.store.entry(aid))!.liveFrom!;
      expect(from_.key, b.manifest.author.key);
      expect(from_.who, 'Ed');
      expect(from_.origin, remote);

      // The app's own author is the person here; Ed's code still asks.
      final settings = SettingsStore(File('${tmp.path}/settings.json'));
      final trust = AuthorTrust(
        lineages: me.store,
        settings: settings,
        ownKey: () async => mine,
      );
      expect(await trust.authorOf(aid), isNull);
      expect(await trust.ensure(null, aid), isFalse, reason: 'asks');
      // Once trusted, the next change goes without asking.
      await settings.save(
        AppSettings(
          trustedAuthors: {
            aid: [b.manifest.author.key],
          },
        ),
      );
      expect(await trust.ensure(null, aid), isTrue);
    });

    test('a release brought into the person’s version records what it '
        'published as merged; a plain release after it clears that', () async {
      await linkTo('ra1');
      final out = await me.updater.update(
        aid,
        await v2ByEditor(),
        origin: from('ra1'),
        merged: Merged(
          files: {'index.html': utf('<p>v2 by an editor</p><p>mine</p>')},
          title: 'Plant log',
          capabilities: const {'db': {}},
          note: 'Both.',
        ),
      );
      expect(out, isA<UpdateLanded>());
      final live = (await me.api.getArtifact(aid)).current!.contentSha256;
      var link = (await me.store.entry(aid))!.link!;
      expect(link.takenBundleVersion, 2);
      expect(link.mergedSha256, live);

      final v3 = await BundleReader.read(
        (await makeBundle(
          lineage: lineageA,
          version: 3,
          files: {'index.html': '<p>v3</p>'},
          seedByte: 5,
          name: 'Ed',
        )).bytes,
      );
      expect(
        await me.updater.update(aid, v3, origin: from('ra1')),
        isA<UpdateLanded>(),
      );
      link = (await me.store.entry(aid))!.link!;
      expect(link.takenBundleVersion, 3);
      expect(link.mergedSha256, isNull);
    });

    test('another key is refused without the linked origin', () async {
      await linkTo('ra1');
      final b = await v2ByEditor();
      for (final origin in [null, from('ra2')]) {
        final out = await me.updater.update(aid, b, origin: origin);
        expect(out, isA<UpdateRefused>(), reason: '$origin');
        expect(
          (out as UpdateRefused).message,
          'This file is not an update of this app.',
        );
      }
      expect((await me.store.entry(aid))!.version, 1);
      expect(me.api.callNames, isNot(contains('createDraft')));
    });

    // What a refused update must leave as it was: the live files, the
    // lineage entry (version, key, lineage, releases) and its link.
    Future<Object> held() async {
      final e = (await me.store.entry(aid))!;
      final live = await me.api.getArtifact(aid);
      return jsonEncode({
        'live': live.current!.contentSha256,
        'liveVersion': live.artifact.currentVersion,
        'version': e.version,
        'key': e.key,
        'lineage': e.lineage,
        'released': e.released,
        'link': e.link?.toJson(),
      });
    }

    void expectNotUpdate(UpdateOutcome out, Object reason) {
      expect(out, isA<UpdateRefused>(), reason: '$reason');
      expect(
        (out as UpdateRefused).message,
        'This file is not an update of this app.',
        reason: '$reason',
      );
    }

    test('a linked app refuses a bundle of another lineage from its own '
        'public copy, whatever key signed it', () async {
      await linkTo('ra1');
      final before = await held();
      for (final seedByte in [9, 5]) {
        final b = await BundleReader.read(
          (await makeBundle(
            lineage: 'l_bbbbbbbbbbbbbbbbbbbbbbbbbb',
            version: 2,
            files: {'index.html': '<p>another app</p>'},
            seedByte: seedByte,
          )).bytes,
        );
        final out = await me.updater.update(aid, b, origin: from('ra1'));
        expectNotUpdate(out, 'key seed $seedByte');
      }
      expect(await held(), before);
      expect(me.api.callNames, isNot(contains('createDraft')));
      expect(me.api.callNames, isNot(contains('publish')));
    });

    test('a linked app refuses a bundle claiming another public copy: '
        'another artifact or another host', () async {
      await linkTo('ra1');
      final before = await held();
      final b = await v2ByEditor();
      final elsewhere = {
        'another artifact': from('ra2'),
        'another host': BundleOrigin(
          remoteOrigin: 'https://other.example',
          remoteArtifactId: 'ra1',
          remoteVersion: 2,
          head: from('ra1').head,
        ),
      };
      for (final MapEntry(key: what, value: origin) in elsewhere.entries) {
        final out = await me.updater.update(aid, b, origin: origin);
        expectNotUpdate(out, what);
      }
      expect(await held(), before);
      expect(me.api.callNames, isNot(contains('createDraft')));
      expect(me.api.callNames, isNot(contains('publish')));
    });
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

  test('a failed live migration rolls back to a base whose icon the icon '
      'rules now refuse: restored as history, with its declaration', () async {
    // The live version holds an icon an older build published, which a
    // publish now refuses.
    const old =
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 9">'
        '<title>A & B</title><rect width="9" height="9"/></svg>';
    expect(iconProblem(old), isNotNull);
    me.api.checkIcons = false;
    await me.api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        entries: {
          'index.html': FileEntry.text('<p>v1</p>'),
          'icon.svg': FileEntry.text(old, type: 'image/svg+xml'),
        },
      ),
    );
    me.api.checkIcons = true;
    final b = await v2(migrate: true);
    final updater = Updater(
      api: me.api,
      store: me.store,
      inspector: _Writing(me.inspector, () async {
        me.api.putDoc(aid, 'plants/p1', {'name': 'FERN', 'v': 2});
      }),
      baseUrl: () => Uri.parse('http://127.0.0.1:9/'),
      snapshots: me.snapshots,
    );
    final out = await updater.update(aid, b);
    expect(out, isA<UpdateRolledBack>());
    expect((out as UpdateRolledBack).restored, isTrue);
    expect(await docs(), {
      'plants/p1': {'name': 'fern'},
      'plants/p2': {'name': 'moss'},
    });
    final live = await me.api.getArtifact(aid);
    expect(live.artifact.currentVersion, 4, reason: 'v3 published, v2 back');
    expect(await me.api.readFile(aid, 4, 'icon.svg'), old);
    expect(await me.api.readFile(aid, 4, 'index.html'), '<p>v1</p>');
    expect(live.current!.contentSha256, isNotNull);
    expect(
      live.current!.contentSha256,
      live.versions.firstWhere((v) => v.n == 2).contentSha256,
    );
    expect(live.current!.label, 'Back to v1');
    expect(live.artifact.capabilities, {'db': {}}, reason: "the base's");
    final restore = me.api.calls.lastWhere((c) => c.name == 'restoreVersion');
    expect(restore.args['n'], 2);
    expect(restore.args['ifVersion'], 3);
    expect(me.snapshots.has(aid), isFalse);
  });

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
