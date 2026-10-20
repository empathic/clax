import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_install.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:flutter_test/flutter_test.dart';
import '../support/paths.dart';

/// The shipped bundle and release record, read from the asset files.
class _FileShipped implements ShippedHome {
  _FileShipped({this.edit});

  final HomeRelease Function(HomeRelease r)? edit;
  var reads = 0;

  @override
  Future<Uint8List> bundle() async {
    reads++;
    return File(appPath(AssetShippedHome.bundlePath)).readAsBytesSync();
  }

  @override
  Future<HomeRelease> release() async {
    final r = HomeRelease.parse(
      File(appPath(AssetShippedHome.releasePath)).readAsStringSync(),
    );
    return edit?.call(r) ?? r;
  }
}

void main() {
  late Directory tmp;
  late FakeClaxApi api;
  late LineageStore store;
  late Installer installer;
  late _FileShipped shipped;

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('home-install');
    api = FakeClaxApi();
    await api.registerAppSession(installId: 'i-home', pid: 1);
    store = LineageStore(Directory('${tmp.path}/support'));
    installer = Installer(api: api, store: store, key: homeInstallKey());
    shipped = _FileShipped();
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  Future<HomeSettings> settings() =>
      HomeSettings.load(Directory('${tmp.path}/support'));

  int creates() => api.calls.where((c) => c.name == 'createArtifact').length;

  test('the first run installs the shipped home app once and records its '
      'ID; a second run is a no-op', () async {
    final s = await settings();
    final aid = await ensureHomeApp(s, installer, api, shipped: shipped);
    final d = await api.getArtifact(aid);
    expect(d.artifact.title, 'Home');
    expect(d.artifact.capabilities.keys, containsAll(['home', 'db']));
    expect((await settings()).homeAppId, aid);
    expect(creates(), 1);

    final again = await ensureHomeApp(
      await settings(),
      installer,
      api,
      shipped: shipped,
    );
    expect(again, aid);
    expect(creates(), 1);
    expect(shipped.reads, 1, reason: 'a recorded live app reads no bundle');
  });

  test('a recorded ID whose app was deleted installs it again', () async {
    final s = await settings();
    final first = await ensureHomeApp(s, installer, api, shipped: shipped);
    await api.deleteArtifact(first);
    final second = await ensureHomeApp(
      await settings(),
      installer,
      api,
      shipped: shipped,
    );
    expect(second, isNot(first));
    expect((await settings()).homeAppId, second);
    expect(creates(), 2);
  });

  test('an install cut short before its ID was saved is adopted by lineage, '
      'not made twice', () async {
    // The installer made the app and wrote its lineage entry; the ID
    // never reached home.json.
    final made = (await installer.install(
      await installer.open(await shipped.bundle()),
    )).aid;
    final s = await settings();
    expect(s.homeAppId, isNull);
    final aid = await ensureHomeApp(s, installer, api, shipped: shipped);
    expect(aid, made);
    expect(creates(), 1);
    expect((await settings()).homeAppId, made);
  });

  test('installs with the local daemon only: no call leaves it', () async {
    await ensureHomeApp(await settings(), installer, api, shipped: shipped);
    expect(
      api.calls.map((c) => c.name).toSet(),
      everyElement(
        isIn([
          'registerAppSession',
          'getArtifact',
          'listArtifacts',
          'createArtifact',
          'batchDocs',
        ]),
      ),
    );
  });

  test('a bundle that is not the one release.json names is refused, and '
      'nothing is recorded', () async {
    final s = await settings();
    final wrongKey = _FileShipped(
      edit: (r) => HomeRelease(
        release: r.release,
        hash: r.hash,
        version: r.version,
        lineage: r.lineage,
        key: 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA',
      ),
    );
    await expectLater(
      ensureHomeApp(s, installer, api, shipped: wrongKey),
      throwsA(isA<HomeInstallError>()),
    );
    expect(creates(), 0);
    expect((await settings()).homeAppId, isNull);
  });

  test('a home.json that cannot be written leaves the settings in memory '
      'unchanged', () async {
    // The settings directory's place is taken by a file: the save fails.
    final blocked = File('${tmp.path}/blocked')..writeAsStringSync('');
    final s = await HomeSettings.load(Directory('${blocked.path}/support'));
    await expectLater(
      ensureHomeApp(s, installer, api, shipped: shipped),
      throwsA(isA<FileSystemException>()),
    );
    expect(s.homeAppId, isNull);
  });
}
