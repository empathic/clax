import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/bundle/bundle_reader.dart';
import 'package:clax_app/bundle/exporter.dart';
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/trust.dart';
import 'package:clax_app/clax/api.dart';
import 'package:flutter_test/flutter_test.dart';

import 'services_support.dart';

void main() {
  late Directory tmp;
  late Install me;

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('bs-export');
    me = Install(tmp);
    await me.ready();
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  Future<String> makeApp() async {
    final r = await me.api.createArtifact(
      PublishRequest(
        title: 'Plant Log!',
        icon: 'leaf',
        capabilities: {
          'db': {
            'read': ['a_other'],
          },
          'notify': {},
        },
        entries: {
          'index.html': FileEntry.text('<p>plants</p>'),
          'img/dot.png': FileEntry.bytes(
            Uint8List.fromList([0x89, 0x50, 0, 255]),
            type: 'image/png',
          ),
        },
      ),
    );
    return r.artifact.id;
  }

  test('bundleSlug', () {
    expect(bundleSlug('Plant Log!'), 'plant-log');
    expect(bundleSlug('  ¡¿  '), 'app');
    expect(bundleSlug('a' * 50), 'a' * 40);
  });

  test('first export mints a lineage at v1, binary files intact, no seed, '
      'db.read stripped; an unchanged export keeps the version, a change '
      'bumps it', () async {
    final aid = await makeApp();
    me.api.putDoc(aid, 'plants/p1', {'name': 'fern'});
    final plan = await me.exporter.plan(aid);
    expect(plan.modes, [ExportMode.own]);
    expect(plan.version, 1);
    expect(plan.fileCount, 2);
    expect(plan.docCount, 1);
    expect(plan.dataAllowed, isTrue);

    final out = await me.exporter.export(aid, authorName: 'Alex');
    expect(out.fileName, 'plant-log.clax');
    final b = await BundleReader.read(out.bytes);
    expect(b.manifest.version, 1);
    expect(b.manifest.seed, isNull);
    expect(b.manifest.author.name, 'Alex');
    expect(b.manifest.capabilities, {'db': {}, 'notify': {}});
    expect(b.files['img/dot.png'], [0x89, 0x50, 0, 255]);
    expect(
      b.manifest.contentSha256,
      (await me.api.getArtifact(aid)).current!.contentSha256,
    );
    final e = (await me.store.entry(aid))!;
    expect(e.role, LineageRole.author);
    expect(e.lineage, b.manifest.lineage);
    expect(e.released, [b.manifest.contentSha256]);

    final again = await me.exporter.export(aid);
    expect(again.manifest.version, 1);
    expect(again.manifest.lineage, b.manifest.lineage);

    await me.api.publish(
      aid,
      PublishRequest(ifVersion: 1, files: {'index.html': '<p>more plants</p>'}),
    );
    expect((await me.exporter.plan(aid)).version, 2);
    final v2 = await me.exporter.export(aid);
    expect(v2.manifest.version, 2);
    expect(v2.manifest.lineage, b.manifest.lineage);
  });

  test('export with data carries the documents as seed', () async {
    final aid = await makeApp();
    me.api.putDoc(aid, 'plants/p1', {'name': 'fern'});
    me.api.putDoc(aid, 'plants/p2', {'name': 'moss'});
    final out = await me.exporter.export(aid, includeData: true);
    final b = await BundleReader.read(out.bytes);
    expect(b.manifest.seed!.docs, 2);
    expect([for (final d in b.seed!) d.path], ['plants/p1', 'plants/p2']);
  });

  test('re-share sends the kept bundle byte for byte; a modified copy can '
      'fork under this key with forked_from', () async {
    final w = await makeBundle(lineage: lineageA, version: 3);
    final aid = await me.installer.install(await me.installer.open(w.bytes));

    var plan = await me.exporter.plan(aid);
    expect(plan.modes, [ExportMode.reshare]);
    expect(plan.version, 3);
    final shared = await me.exporter.export(aid, mode: ExportMode.reshare);
    expect(shared.bytes, w.bytes);

    await me.api.publish(
      aid,
      PublishRequest(ifVersion: 1, files: {'index.html': '<p>mine</p>'}),
    );
    plan = await me.exporter.plan(aid);
    expect(plan.modes, [ExportMode.reshare, ExportMode.fork]);
    expect(plan.modified, isTrue);

    final fork = await me.exporter.export(aid, mode: ExportMode.fork);
    final m = fork.manifest;
    expect(m.lineage, isNot(lineageA));
    expect(m.version, 1);
    expect(m.forkedFrom!.lineage, lineageA);
    expect(m.forkedFrom!.version, 3);
    expect(m.forkedFrom!.key, w.manifest.author.key);
    expect(m.author.key, (await me.key.existing())!.key);
    final e = (await me.store.entry(aid))!;
    expect(e.role, LineageRole.author);
    expect(e.upstream!.lineage, lineageA);
    expect(e.fromBundle, isTrue);

    // The original's later bundle now installs separately.
    final v4 = await makeBundle(lineage: lineageA, version: 4);
    expect(
      (await me.installer.open(v4.bytes)).decision.answer,
      TrustAnswer.forked,
    );
  });

  test('an own export of an installed app is refused', () async {
    final w = await makeBundle(lineage: lineageA);
    final aid = await me.installer.install(await me.installer.open(w.bytes));
    await expectLater(me.exporter.export(aid), throwsA(isA<BundleFailure>()));
  });
}
