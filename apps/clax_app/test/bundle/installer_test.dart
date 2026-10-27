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
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/remote/models.dart';
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

  test('a bundle got from a remote installs linked, and its next version '
      'under another key opens as a linked update', () async {
    const remote = 'https://clax.example';
    const by = PublishedBy(name: 'Sam');
    BundleOrigin from(int n) => BundleOrigin(
      remoteOrigin: remote,
      remoteArtifactId: 'ra1',
      remoteVersion: n,
      head: RemoteHead(
        artifactId: 'ra1',
        url: '$remote/a/ra1',
        title: 'Plant log',
        version: n,
        contentSha256: 'sha256:${'0' * 64}',
        publishedBy: by,
        owner: by,
        comments: false,
      ),
    );
    final v1 = await makeBundle(lineage: lineageA);
    final opened = await me.installer.open(v1.bytes, origin: from(1));
    expect(opened.origin?.remoteArtifactId, 'ra1');
    expect(opened.decision.answer, TrustAnswer.install);
    const link = AppLink(
      remoteOrigin: remote,
      remoteId: 'r1',
      remoteArtifactId: 'ra1',
      url: '$remote/a/ra1',
      baseRemoteVersion: 1,
      takenBundleVersion: 1,
      role: AppLink.recipient,
    );
    final aid = (await me.installer.install(opened, link: link)).aid;
    final e = (await me.store.entry(aid))!;
    expect(e.link?.toJson(), link.toJson());

    final v2 = await makeBundle(lineage: lineageA, version: 2, seedByte: 5);
    final unlinked = await me.installer.open(v2.bytes);
    expect(unlinked.decision.answer, TrustAnswer.foreign);
    final linked = await me.installer.open(v2.bytes, origin: from(2));
    expect(linked.decision.answer, TrustAnswer.update);
    expect(linked.decision.linked, isTrue);
    expect(linked.decision.aid, aid);
  });

  test('a link the bundle was not got from is refused, with nothing '
      'made', () async {
    const remote = 'https://clax.example';
    const link = AppLink(
      remoteOrigin: remote,
      remoteArtifactId: 'ra1',
      url: '$remote/a/ra1',
      baseRemoteVersion: 1,
      role: AppLink.recipient,
    );
    final w = await makeBundle(lineage: lineageA);
    final fromFile = await me.installer.open(w.bytes);
    await expectLater(
      me.installer.install(fromFile, link: link),
      throwsA(isA<BundleFailure>()),
    );
    const by = PublishedBy(name: 'Sam');
    final elsewhere = await me.installer.open(
      w.bytes,
      origin: const BundleOrigin(
        remoteOrigin: remote,
        remoteArtifactId: 'ra2',
        remoteVersion: 1,
        head: RemoteHead(
          artifactId: 'ra2',
          url: '$remote/a/ra2',
          title: 'Plant log',
          version: 1,
          contentSha256: 'sha256:0',
          publishedBy: by,
          owner: by,
          comments: false,
        ),
      ),
    );
    await expectLater(
      me.installer.install(elsewhere, link: link),
      throwsA(isA<BundleFailure>()),
    );
    expect(await me.api.listArtifacts(), isEmpty);
    expect(await me.store.all(), isEmpty);
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

  test('an install approves the version its person accepted', () async {
    final w = await makeBundle(lineage: lineageA);
    final aid = (await me.installer.install(await me.installer.open(w.bytes)))
        .aid;
    expect(me.api.approved, [(aid, 1, FakeClaxApi.pendingSha256(aid, 1))]);
  });

  test('an install with nothing pending approves nothing', () async {
    me.api.pendsApproval = false;
    final w = await makeBundle(lineage: lineageA);
    await me.installer.install(await me.installer.open(w.bytes));
    expect(me.api.callNames, isNot(contains('approveCapabilities')));
  });

  test('a refused approval leaves the install landed', () async {
    me.api.failNext(
      'approveCapabilities',
      ClaxApiError(status: 403, code: 'admin_only', message: 'owner only'),
    );
    final w = await makeBundle(lineage: lineageA);
    final aid = (await me.installer.install(await me.installer.open(w.bytes)))
        .aid;
    expect(me.api.approved, isEmpty);
    expect((await me.api.getArtifact(aid)).current!.n, 1);
    expect(await me.store.entry(aid), isNotNull);
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
      'makes it pass, and says so', () async {
    expect(iconProblem(fixable), isNotNull);
    final w = await makeBundle(
      lineage: lineageA,
      files: {'index.html': '<p>hi</p>', 'icon.svg': fixable},
    );
    final opened = await me.installer.open(w.bytes);
    final got = await me.installer.install(opened);
    final change = PublishedFiles.of(opened.bundle).iconChange;
    final svg = await me.api.readFile(got.aid, 1, 'icon.svg');
    expect(svg, cleanIcon(fixable));
    expect(iconProblem(svg!), isNull);
    expect(change, 'Its icon broke Clax’s icon rules, so Clax redrew it.');
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
      'without it, and says so', () async {
    expect(cleanIcon(hopeless), isNull);
    final w = await makeBundle(
      lineage: lineageA,
      files: {'index.html': '<p>hi</p>', 'icon.svg': hopeless},
    );
    final opened = await me.installer.open(w.bytes);
    final got = await me.installer.install(opened);
    final change = PublishedFiles.of(opened.bundle).iconChange;
    final d = await me.api.getArtifact(got.aid);
    expect(d.current!.files.keys, ['index.html']);
    expect(change, 'Its icon broke Clax’s icon rules, so Clax left it out.');
    expect(d.current!.note, 'Installed v1 of Plant log from Sam; its icon '
        'was left out (icon rule view_box)');
  });

  // An icon that passes the rules but for one Latin-1 byte (é as 0xE9),
  // which the core refuses as not UTF-8.
  final latin1Icon = latin1.encode(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 9">'
    '<title>Caf\u00e9</title><rect width="9" height="9"/></svg>',
  );

  test('a bundle icon that is not UTF-8 is installed re-read as Latin-1 '
      'when that passes the icon rules, and says so', () async {
    expect(() => utf8.decode(latin1Icon), throwsFormatException);
    final w = await makeBundle(
      lineage: lineageA,
      files: {'index.html': '<p>hi</p>'},
      rawFiles: {'icon.svg': latin1Icon},
    );
    final opened = await me.installer.open(w.bytes);
    final got = await me.installer.install(opened);
    final change = PublishedFiles.of(opened.bundle).iconChange;
    final svg = await me.api.readFile(got.aid, 1, 'icon.svg');
    expect(svg, contains('<title>Caf\u00e9</title>'));
    expect(iconProblem(svg!), isNull);
    expect(change, 'Its icon broke Clax’s icon rules, so Clax redrew it.');
    final d = await me.api.getArtifact(got.aid);
    expect(d.current!.note, 'Installed v1 of Plant log from Sam; its icon '
        'was redrawn (icon rule not_xml)');
  });

  test('a bundle icon that is neither UTF-8 nor a passing icon as Latin-1 '
      'is left out', () async {
    final bad = [...latin1.encode('<svg viewBox="0 0 9 8"><title>'), 0xE9,
        ...latin1.encode('</title></svg>')];
    final w = await makeBundle(
      lineage: lineageA,
      files: {'index.html': '<p>hi</p>'},
      rawFiles: {'icon.svg': bad},
    );
    final opened = await me.installer.open(w.bytes);
    final got = await me.installer.install(opened);
    final change = PublishedFiles.of(opened.bundle).iconChange;
    final d = await me.api.getArtifact(got.aid);
    expect(d.current!.files.keys, ['index.html']);
    expect(change, 'Its icon broke Clax’s icon rules, so Clax left it out.');
    expect(d.current!.note, 'Installed v1 of Plant log from Sam; its icon '
        'was left out (icon rule not_xml)');
  });

  test('an icon within the rules is installed byte for byte', () async {
    const good =
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 9 9">'
        '<rect width="9" height="9"/></svg>';
    final w = await makeBundle(
      lineage: lineageA,
      files: {'index.html': '<p>hi</p>', 'icon.svg': good},
    );
    final opened = await me.installer.open(w.bytes);
    final got = await me.installer.install(opened);
    final change = PublishedFiles.of(opened.bundle).iconChange;
    expect(change, isNull);
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
    final events = StreamController<StreamEvent>();
    final sub = forgetDeleted(events.stream, me.store, snapshots: me.snapshots);
    await me.snapshots.take(
      aid,
      const SnapshotMeta(base: 1, contentSha256: 'x', title: 't', backOn: 1),
    );
    events.add(StreamEvent(topic: 'gallery', type: 'artifact_deleted', data: {'artifact_id': aid}));
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
