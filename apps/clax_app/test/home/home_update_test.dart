import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/home/home_install.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/home/home_update.dart';
import 'package:clax_app/ui/bundle_sheets.dart' show UpdateChoice;
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../bundle/services_support.dart';

const _lineage = 'l_hhhhhhhhhhhhhhhhhhhhhhhhhh';

/// A shipped home bundle at [version], released as [release].
class _Shipped implements ShippedHome {
  _Shipped(this.bytes, this._release);

  final Uint8List bytes;
  final HomeRelease _release;

  static Future<_Shipped> at(int version, String release) async {
    final w = await makeBundle(
      lineage: _lineage,
      version: version,
      title: 'Home',
      capabilities: const {'home': {}, 'db': {}},
      files: {'index.html': '<p>home v$version</p>'},
    );
    final m = w.manifest;
    return _Shipped(
      w.bytes,
      HomeRelease(
        release: release,
        hash: m.contentSha256,
        version: m.version,
        lineage: m.lineage,
        key: m.author.key,
      ),
    );
  }

  @override
  Future<Uint8List> bundle() async => bytes;

  @override
  Future<HomeRelease> release() async => _release;
}

void main() {
  late Directory tmp;
  late Install me;
  late HomeSettings settings;
  late String aid;
  late List<OpenedBundle> asked;
  UpdateChoice? answer;

  setUp(() async {
    tmp = Directory.systemTemp.createTempSync('home-update');
    me = Install(tmp);
    await me.ready();
    settings = await HomeSettings.load(Directory('${tmp.path}/support'));
    aid = await ensureHomeApp(
      settings,
      me.installer,
      me.api,
      shipped: await _Shipped.at(1, '1.0.0'),
    );
    me.api.putDoc(aid, 'layout/order', {
      'ids': ['b', 'a'],
    });
    asked = [];
    answer = null;
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  Future<HomeUpdateResult> check(ShippedHome shipped) => updateHomeApp(
    settings,
    me.installer,
    shipped: shipped,
    update: me.updater.update,
    ask: (opened) async {
      asked.add(opened);
      return answer;
    },
  );

  Future<String> liveIndex() async {
    final d = await me.api.getArtifact(aid);
    return String.fromCharCodes(
      (await me.api.readFileBytes(aid, d.current!.n, 'index.html'))!,
    );
  }

  /// The person changes the home app by commenting: a new version of
  /// their own.
  Future<void> changeIt() async {
    final d = await me.api.getArtifact(aid);
    await me.api.publish(
      aid,
      PublishRequest(
        ifVersion: d.artifact.currentVersion,
        entries: {
          'index.html': FileEntry.bytes(
            utf('<p>sorted alphabetically</p>'),
            type: 'text/html; charset=utf-8',
          ),
        },
      ),
    );
  }

  int publishes() => me.api.calls.where((c) => c.name == 'publish').length;

  test('the same release does nothing', () async {
    me.api.calls.clear();
    expect(await check(await _Shipped.at(1, '1.0.0')), HomeUpdateResult.none);
    expect(publishes(), 0);
    expect(asked, isEmpty);
  });

  test('a new release over an unmodified home app updates it silently, '
      'keeping its documents', () async {
    me.api.calls.clear();
    final out = await check(await _Shipped.at(2, '1.1.0'));
    expect(out, HomeUpdateResult.updated);
    expect(asked, isEmpty);
    expect(await liveIndex(), '<p>home v2</p>');
    expect((await me.store.entry(aid))!.version, 2);
    expect((await me.api.getDoc(aid, 'layout/order'))!.data, {
      'ids': ['b', 'a'],
    });
    expect(settings.homeAppId, aid);
  });

  test('a new release over a changed home app asks; Keep mine is not '
      'asked again for that release, and is for the next', () async {
    await changeIt();
    answer = UpdateChoice.keepMine;
    expect(await check(await _Shipped.at(2, '1.1.0')), HomeUpdateResult.kept);
    expect(asked, hasLength(1));
    expect(asked.single.decision.modified, isTrue);
    expect(await liveIndex(), '<p>sorted alphabetically</p>');
    expect(
      (await HomeSettings.load(Directory('${tmp.path}/support'))).keptRelease,
      '1.1.0',
    );

    expect(await check(await _Shipped.at(2, '1.1.0')), HomeUpdateResult.kept);
    expect(asked, hasLength(1), reason: 'asked once per release');

    answer = null;
    expect(await check(await _Shipped.at(3, '1.2.0')), HomeUpdateResult.none);
    expect(asked, hasLength(2), reason: 'the next release asks again');
    expect(await liveIndex(), '<p>sorted alphabetically</p>');
  });

  testWidgets('the home update sheet says Keep mine skips only this '
      'update', (tester) async {
    await tester.runAsync(() async {
      await changeIt();
      await check(await _Shipped.at(2, '1.1.0'));
    });
    final opened = asked.single;
    final answers = <UpdateChoice?>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => Scaffold(
            body: TextButton(
              onPressed: () async =>
                  answers.add(await askHomeUpdate(context, opened)),
              child: const Text('show'),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('show'));
    await tester.pumpAndSettle();
    expect(find.textContaining('stops updates for this app'), findsNothing);
    expect(
      find.textContaining('skips this update; the next one asks again.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Keep mine'));
    await tester.pumpAndSettle();
    expect(answers, [UpdateChoice.keepMine]);
  });

  test('Update replaces the changed home app and keeps its data', () async {
    await changeIt();
    answer = UpdateChoice.update;
    final out = await check(await _Shipped.at(2, '1.1.0'));
    expect(out, HomeUpdateResult.updated);
    expect(asked, hasLength(1));
    expect(await liveIndex(), '<p>home v2</p>');
    expect((await me.api.getDoc(aid, 'layout/order'))!.data, {
      'ids': ['b', 'a'],
    });
    expect(settings.keptRelease, isNull);
  });

  test('an update whose check fails leaves the home app as it was', () async {
    me.inspector.answer(
      me.inspector.clean(errors: [(kind: 'error', text: 'boom')]),
    );
    final out = await check(await _Shipped.at(2, '1.1.0'));
    expect(out, HomeUpdateResult.failed);
    expect(await liveIndex(), '<p>home v1</p>');
  });

  test('no home app yet: nothing to update', () async {
    settings.homeAppId = null;
    me.api.calls.clear();
    expect(await check(await _Shipped.at(2, '1.1.0')), HomeUpdateResult.none);
    expect(publishes(), 0);
  });
}
