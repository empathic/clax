import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/bundle/trust.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_install.dart';
import 'package:clax_app/home/home_recovery.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:flutter_test/flutter_test.dart';

/// The shipped bundle and release record, read from the asset files.
class _FileShipped implements ShippedHome {
  @override
  Future<Uint8List> bundle() async =>
      File(AssetShippedHome.bundlePath).readAsBytesSync();

  @override
  Future<HomeRelease> release() async =>
      HomeRelease.parse(File(AssetShippedHome.releasePath).readAsStringSync());
}

void main() {
  late Directory tmp;
  late FakeClaxApi api;
  late LineageStore store;
  late Installer installer;
  late HomeSettings settings;
  late String aid;
  final shipped = _FileShipped();

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('home-recovery');
    api = FakeClaxApi();
    await api.registerAppSession(installId: 'i-home', pid: 1);
    store = LineageStore(Directory('${tmp.path}/support'));
    installer = Installer(api: api, store: store, key: homeInstallKey());
    settings = await HomeSettings.load(Directory('${tmp.path}/support'));
    aid = await ensureHomeApp(settings, installer, api, shipped: shipped);
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  /// A change the person made by commenting: a new index.html and an
  /// extra file, as the agent would publish it.
  Future<void> change(String html) async {
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: api.currentVersion(aid),
        entries: {
          'index.html': FileEntry.text(html),
          'extra.js': FileEntry.text('1'),
        },
      ),
    );
  }

  test('Undo restores the version before the latest', () async {
    final shippedHtml = api.filesOf(aid, 1)['index.html'];
    await change('<p>broken</p>');
    expect(api.currentVersion(aid), 2);

    await undoLast(api, aid);
    final restore = api.calls.lastWhere((c) => c.name == 'restoreVersion');
    expect(restore.args, {'aid': aid, 'n': 1, 'ifVersion': 2});
    expect(api.currentVersion(aid), 3);
    expect(api.filesOf(aid, 3)['index.html'], shippedHtml);
  });

  test('Undo with no earlier version says so', () async {
    await expectLater(undoLast(api, aid), throwsA(isA<HomeRecoveryError>()));
    expect(api.calls.where((c) => c.name == 'restoreVersion'), isEmpty);
  });

  test('Undo again steps back one more version, across a restart, until '
      'there is none', () async {
    final good = api.filesOf(aid, 1)['index.html'];
    await change('<p>broken 2</p>');
    await change('<p>broken 3</p>');
    expect(api.currentVersion(aid), 3);

    await undoLast(api, aid, settings: settings);
    expect(api.filesOf(aid, 4)['index.html'], '<p>broken 2</p>');

    // A restart: the chain is read back from home.json.
    settings = await HomeSettings.load(Directory('${tmp.path}/support'));
    await undoLast(api, aid, settings: settings);
    expect(api.currentVersion(aid), 5);
    expect(api.filesOf(aid, 5)['index.html'], good);
    expect(
      api.calls
          .where((c) => c.name == 'restoreVersion')
          .map((c) => c.args['n']),
      [2, 1],
    );

    await expectLater(
      undoLast(api, aid, settings: settings),
      throwsA(isA<HomeRecoveryError>()),
    );
    expect(api.currentVersion(aid), 5);
  });

  test('A change landing after an Undo starts the chain again from the '
      'latest', () async {
    await change('<p>broken 2</p>');
    await change('<p>broken 3</p>');
    await undoLast(api, aid, settings: settings); // v4 = v2
    await change('<p>fixed 5</p>');
    await undoLast(api, aid, settings: settings);
    expect(api.calls.lastWhere((c) => c.name == 'restoreVersion').args['n'], 4);
  });

  test('Reset ends the chain', () async {
    await change('<p>broken 2</p>');
    await undoLast(api, aid, settings: settings); // v3 = v1
    await reset(installer, api, aid, shipped: shipped, settings: settings);
    expect(settings.undo, isNull);
    expect(
      (await HomeSettings.load(Directory('${tmp.path}/support'))).undo,
      isNull,
    );
    await undoLast(api, aid, settings: settings);
    expect(api.calls.lastWhere((c) => c.name == 'restoreVersion').args['n'], 3);
  });

  test('Reset publishes the shipped files as the next version, documents '
      'and threads untouched, and the lineage reads it as the shipped '
      'release', () async {
    final shippedFiles = api.filesOf(aid, 1);
    final shippedHash = api.versionOf(aid, 1).contentSha256;
    api.putDoc(aid, 'data/notes/n1', {'text': 'mine'});
    final tid = api.addThread(aid, 'Make the tiles bigger');
    final docsBefore = await api.listDocs(aid, 'data/notes');
    await store.stopUpdates(aid);
    settings.keptRelease = '0.0.1';
    await change('<p>broken</p>');
    final callsBefore = api.calls.length;

    final r = await reset(
      installer,
      api,
      aid,
      shipped: shipped,
      settings: settings,
    );
    expect(r.version.n, 3);
    expect(api.currentVersion(aid), 3);
    expect(api.filesOf(aid, 3), shippedFiles, reason: 'extra.js removed');
    expect(api.versionOf(aid, 3).contentSha256, shippedHash);
    expect(api.capabilitiesOf(aid).keys, containsAll(['home', 'db']));

    expect(
      api.calls.skip(callsBefore).map((c) => c.name),
      isNot(anyOf(contains('batchDocs'), contains('deleteDoc'))),
      reason: 'no document is written',
    );
    final docsAfter = await api.listDocs(aid, 'data/notes');
    expect(
      docsAfter.map((d) => (d.path, d.data, d.version)),
      docsBefore.map((d) => (d.path, d.data, d.version)),
    );
    expect((await api.threads(aid)).map((t) => t.id), contains(tid));

    final entry = (await store.entry(aid))!;
    expect(entry.updatesStopped, isFalse);
    expect(entry.released.last, shippedHash);
    expect(
      entry.isModified(
        contentSha256: shippedHash!,
        capabilities: shareableCapabilities(api.capabilitiesOf(aid)),
      ),
      isFalse,
    );
    expect(settings.keptRelease, isNull);
    expect(
      (await HomeSettings.load(Directory('${tmp.path}/support'))).keptRelease,
      isNull,
    );

    // Opening the shipped bundle again finds the app current: no update,
    // no second install.
    final opened = await installer.open(await shipped.bundle());
    expect(opened.decision.answer, TrustAnswer.current);
    expect(opened.decision.aid, aid);
  });

  test(
    'Reset of an app with no lineage entry writes the installed one',
    () async {
      await store.forget(aid);
      // A later start: the store no longer holds the forgotten ID back.
      store = LineageStore(Directory('${tmp.path}/support'));
      installer = Installer(api: api, store: store, key: homeInstallKey());
      await change('<p>broken</p>');
      await reset(installer, api, aid, shipped: shipped);
      final entry = (await store.entry(aid))!;
      expect(entry.role, LineageRole.installed);
      expect(entry.fromBundle, isTrue);
      final opened = await installer.open(await shipped.bundle());
      expect(opened.decision.answer, TrustAnswer.current);
    },
  );

  test('HomeRecovery tells its listeners after each that lands, not after '
      'a failure', () async {
    final rec = HomeRecovery(
      api: api,
      installer: installer,
      settings: settings,
      shipped: shipped,
    );
    var told = 0;
    rec.addListener(() => told++);
    await expectLater(rec.undoLast(), throwsA(isA<HomeRecoveryError>()));
    expect(told, 0);
    await change('<p>broken</p>');
    await rec.undoLast();
    expect(told, 1);
    await rec.reset();
    expect(told, 2);
  });
}
