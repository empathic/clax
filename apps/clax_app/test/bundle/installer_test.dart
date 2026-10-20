import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/agent/icon_svg.dart';
import 'package:clax_app/bundle/bundle_writer.dart';
import 'package:clax_app/bundle/doc_snapshot.dart';
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/manifest.dart';
import 'package:clax_app/bundle/trust.dart';
import 'package:clax_app/clax/api.dart';
import 'package:flutter_test/flutter_test.dart';

import 'services_support.dart';

void main() {
  late Directory tmp;
  late Install me;

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('bs-install');
    me = Install(tmp);
    await me.ready();
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  test('a damaged file is refused in one sentence', () async {
    final w = await makeBundle(lineage: lineageA);
    final bytes = Uint8List.fromList(w.bytes)..[w.bytes.length ~/ 2] ^= 1;
    await expectLater(me.installer.open(bytes), throwsA(isA<BundleFailure>()));
    await expectLater(
      me.installer.open(utf('not a zip at all, just text')),
      throwsA(
        isA<BundleFailure>().having((f) => f.code, 'code', 'not_a_bundle'),
      ),
    );
  });

  test('installs a new lineage: files, content hash, seed, lineage, '
      'kept bundle', () async {
    final w = await makeBundle(
      lineage: lineageA,
      version: 2,
      files: {'index.html': '<p>hi</p>', 'app.js': 'let x = 1;'},
      seed: [
        for (var i = 0; i < 60; i++) SeedDoc('plants/p$i', {'n': i}),
      ],
    );
    final opened = await me.installer.open(w.bytes);
    expect(opened.decision.answer, TrustAnswer.install);
    expect(opened.who, 'Sam');
    expect(opened.fromSelf, isFalse);

    final aid = (await me.installer.install(opened)).aid;
    final d = await me.api.getArtifact(aid);
    expect(d.artifact.title, 'Plant log');
    expect(d.artifact.icon, 'leaf');
    expect(d.current!.contentSha256, w.manifest.contentSha256);
    expect(d.current!.label, 'v2 from Sam');
    expect(await me.api.readFile(aid, 1, 'app.js'), 'let x = 1;');

    final batches = me.api.calls.where((c) => c.name == 'batchDocs').toList();
    expect(batches, hasLength(2), reason: '50 writes a call');
    final docs = [await for (final d in ownerDocs(me.api, aid)) d];
    expect(docs, hasLength(60));

    final e = (await me.store.entry(aid))!;
    expect(e.role, LineageRole.installed);
    expect(e.lineage, lineageA);
    expect(e.version, 2);
    expect(e.released, [w.manifest.contentSha256]);
    expect(e.fromBundle, isTrue);
    expect(await me.store.keptBundle(aid), w.bytes);

    // Opening it again: current.
    final again = await me.installer.open(w.bytes);
    expect(again.decision.answer, TrustAnswer.current);
    expect(again.decision.aid, aid);
  });

  // An icon the app's old check passed and the core now refuses (a bare
  // &), which cleanIcon writes out well-formed; and one it cannot draw.
  const fixable =
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 9">'
      '<title>A & B</title><rect width="9" height="9"/></svg>';
  const hopeless =
      '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 8">'
      '<rect width="9" height="8"/></svg>';

  test('a bundle icon the icon rules refuse is installed cleaned when that '
      'makes it pass, and the result says so', () async {
    expect(iconProblem(fixable), isNotNull);
    final w = await makeBundle(
      lineage: lineageA,
      files: {'index.html': '<p>hi</p>', 'icon.svg': fixable},
    );
    final got = await me.installer.install(await me.installer.open(w.bytes));
    final svg = await me.api.readFile(got.aid, 1, 'icon.svg');
    expect(svg, cleanIcon(fixable));
    expect(iconProblem(svg!), isNull);
    expect(got.iconChange, startsWith('Its icon was redrawn without what an icon '
        'may not hold: icon.svg is not well-formed XML'));
    final d = await me.api.getArtifact(got.aid);
    expect(d.current!.note, 'Installed v1 of Plant log from Sam; its icon '
        'was redrawn (icon rule not_xml)');
    final hash = d.current!.contentSha256!;
    expect(hash, isNot(w.manifest.contentSha256));
    final e = (await me.store.entry(got.aid))!;
    expect(e.released, [hash, w.manifest.contentSha256]);
    // Opening the same bundle again: current, and the app unmodified.
    final again = await me.installer.open(w.bytes);
    expect(again.decision.answer, TrustAnswer.current);
    expect(
      e.isModified(contentSha256: hash, capabilities: e.capabilities),
      isFalse,
    );
  });

  test('a bundle icon cleaning cannot save is left out: the app installs '
      'without it, and the result says so', () async {
    expect(cleanIcon(hopeless), isNull);
    final w = await makeBundle(
      lineage: lineageA,
      files: {'index.html': '<p>hi</p>', 'icon.svg': hopeless},
    );
    final got = await me.installer.install(await me.installer.open(w.bytes));
    final d = await me.api.getArtifact(got.aid);
    expect(d.current!.files.keys, ['index.html']);
    expect(got.iconChange, 'Its icon was left out: ${iconProblem(hopeless)!.message}');
    expect(d.current!.note, 'Installed v1 of Plant log from Sam; its icon '
        'was left out (icon rule view_box)');
  });

  test('an icon within the rules is installed byte for byte', () async {
    const good =
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 9">'
        '<rect width="9" height="9"/></svg>';
    final w = await makeBundle(
      lineage: lineageA,
      files: {'index.html': '<p>hi</p>', 'icon.svg': good},
    );
    final got = await me.installer.install(await me.installer.open(w.bytes));
    expect(got.iconChange, isNull);
    expect(await me.api.readFile(got.aid, 1, 'icon.svg'), good);
    final d = await me.api.getArtifact(got.aid);
    expect(d.current!.contentSha256, w.manifest.contentSha256);
  });

  test(
    'a daemon that reports another content hash: the app is deleted',
    () async {
      me.api.reportWrongContentHash = true;
      final w = await makeBundle(lineage: lineageA);
      final opened = await me.installer.open(w.bytes);
      await expectLater(
        me.installer.install(opened),
        throwsA(isA<BundleFailure>().having((f) => f.code, 'code', 'bad_file')),
      );
      expect(me.api.callNames, contains('deleteArtifact'));
      expect(await me.api.listArtifacts(), isEmpty);
      expect(await me.store.all(), isEmpty);
    },
  );

  test('invalid_capabilities reads "needs a newer Clax"', () async {
    me.api.failNext(
      'createArtifact',
      const ClaxApiError(
        status: 400,
        code: 'invalid_capabilities',
        message: 'unknown capability "teleport"',
      ),
    );
    final opened = await me.installer.open(
      (await makeBundle(lineage: lineageA)).bytes,
    );
    await expectLater(
      me.installer.install(opened),
      throwsA(
        isA<BundleFailure>().having(
          (f) => f.message,
          'message',
          'This app needs a newer Clax.',
        ),
      ),
    );
  });

  test('a later version of an installed app is an update with its live '
      'state; a deleted app is forgotten', () async {
    final v1 = await makeBundle(lineage: lineageA);
    final aid = (await me.installer.install(await me.installer.open(v1.bytes))).aid;
    final v2 = await makeBundle(
      lineage: lineageA,
      version: 2,
      files: {'index.html': '<p>v2</p>'},
    );
    final opened = await me.installer.open(v2.bytes);
    expect(opened.decision.answer, TrustAnswer.update);
    expect(opened.decision.modified, isFalse);
    expect(opened.live!.contentSha256, v1.manifest.contentSha256);

    await me.api.deleteArtifact(aid);
    final after = await me.installer.open(v2.bytes);
    expect(after.decision.answer, TrustAnswer.install);
    expect(await me.store.entry(aid), isNull);
  });

  test('forgetDeleted drops the entry, kept bundle and snapshot on '
      'artifact_deleted', () async {
    final aid = (await me.installer.install(
      await me.installer.open((await makeBundle(lineage: lineageA)).bytes),
    )).aid;
    final events = StreamController<ClaxEvent>();
    final sub = forgetDeleted(events.stream, me.store, snapshots: me.snapshots);
    await me.snapshots.take(
      aid,
      const SnapshotMeta(base: 1, contentSha256: 'x', title: 't', backOn: 1),
    );
    events.add(ClaxEvent(type: 'artifact_deleted', data: {'artifact_id': aid}));
    await pumpEventQueue();
    await sub.cancel();
    expect(await me.store.entry(aid), isNull);
    expect(await me.store.keptBundle(aid), isNull);
    expect(me.snapshots.has(aid), isFalse);
  });

  test('the person\'s own bundle, opened after its app was deleted, is '
      'installed as an author lineage', () async {
    final id = await me.key.obtain();
    final w = await BundleWriter.write(
      lineage: lineageA,
      version: 4,
      title: 'Mine',
      iconWord: null,
      capabilities: const {},
      files: {'index.html': BundleFile(utf('<p>m</p>'), type: 'text/html')},
      keySeed: id.seed,
    );
    final opened = await me.installer.open(w.bytes);
    expect(opened.decision.answer, TrustAnswer.install);
    expect(opened.fromSelf, isTrue);
    final aid = (await me.installer.install(opened)).aid;
    final e = (await me.store.entry(aid))!;
    expect(e.role, LineageRole.author);
    expect(e.fromBundle, isFalse);
    expect(e.version, 4);
  });

  test('an install a crash cut short is deleted at start; meanwhile every '
      'app without an entry counts as from a bundle', () async {
    final w = await makeBundle(lineage: lineageA);
    final kept = (await me.installer.install(
      await me.installer.open(
        (await makeBundle(lineage: 'l_cccccccccccccccccccccccccc')).bytes,
      ),
    )).aid;
    final cut = (await me.api.createArtifact(
      PublishRequest(
        title: 'Plant log',
        entries: {
          'index.html': FileEntry.text(
            '<p>v1</p>',
            type: 'text/html; charset=utf-8',
          ),
        },
      ),
    )).artifact.id;
    final marker = installMarker(me.store, lineageA)
      ..parent.createSync(recursive: true)
      ..writeAsStringSync(
        jsonEncode({'content_sha256': w.manifest.contentSha256, 'aid': null}),
      );
    expect(await fromBundleOf(me.store)(cut), isTrue);
    expect(await fromBundleOf(me.store)(kept), isTrue);
    await me.installer.recoverInstalls();
    expect(marker.existsSync(), isFalse);
    expect([for (final a in await me.api.listArtifacts()) a.id], [kept]);
    expect(await fromBundleOf(me.store)('a_other'), isFalse);
  });

  test('fromBundleOf reads the entry', () async {
    final aid = (await me.installer.install(
      await me.installer.open((await makeBundle(lineage: lineageA)).bytes),
    )).aid;
    final f = fromBundleOf(me.store);
    expect(await f(aid), isTrue);
    expect(await f('a_other'), isFalse);
  });

  test('shareableCapabilities drops db.read and db.share only', () {
    expect(
      shareableCapabilities({
        'db': {
          'read': ['a_x'],
          'share': {},
          'users': true,
        },
        'net': {
          'hosts': ['x.com'],
        },
      }),
      {
        'db': {'users': true},
        'net': {
          'hosts': ['x.com'],
        },
      },
    );
  });
}
