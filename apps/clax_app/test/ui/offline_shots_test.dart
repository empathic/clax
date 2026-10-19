// Renders work waiting for the connection: a launcher with a make waiting
// beside an interrupted one, and a thread page whose change waits, on a
// phone, light and dark, for a person to look at. Off by default; run with
//   CLAX_O1_SHOTS=1 flutter test --update-goldens test/ui/offline_shots_test.dart
// and look at target/flutter-app/o1/*.png.
import 'dart:async';
import 'dart:io';

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

final _on = Platform.environment['CLAX_O1_SHOTS'] == '1';
// Goldens resolve from this file.
const _out = '../../../../target/flutter-app/o1';

Future<void> _fonts() async {
  final root = Platform.environment['FLUTTER_ROOT']!;
  final dir = '$root/bin/cache/artifacts/material_fonts';
  Future<ByteData> f(String path) async =>
      ByteData.sublistView(await File(path).readAsBytes());
  await (FontLoader('Roboto')
        ..addFont(f('$dir/Roboto-Regular.ttf'))
        ..addFont(f('$dir/Roboto-Medium.ttf'))
        ..addFont(f('$dir/Roboto-Bold.ttf')))
      .load();
  await (FontLoader(
    'MaterialIcons',
  )..addFont(f('$dir/MaterialIcons-Regular.otf'))).load();
}

void main() {
  for (final dark in [false, true]) {
    final scheme = dark ? 'dark' : 'light';
    Widget app(Widget home) => MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: claxTheme(Brightness.light, defaultTargetPlatform),
      darkTheme: claxTheme(Brightness.dark, defaultTargetPlatform),
      themeMode: dark ? ThemeMode.dark : ThemeMode.light,
      home: home,
    );

    Future<void> phone(WidgetTester tester) async {
      await tester.runAsync(_fonts);
      tester.view.physicalSize = const Size(412 * 2, 915 * 2);
      tester.view.devicePixelRatio = 2;
      addTearDown(tester.view.reset);
    }

    testWidgets(
      'launcher with a make waiting for the connection, $scheme',
      skip: !_on,
      (tester) async {
        await phone(tester);
        final makes = MakeService((r, cancelled, stage) {
          if (r.startsWith('A packing')) stage(MakeStage.offline);
          return Completer<String>().future;
        });
        addTearDown(makes.dispose);
        makes.lost('A water reminder every two hours');
        makes.start('A packing list for trips');
        final api = FakeClaxApi();
        api.seedArtifact('Notes', {'index.html': '<p>n</p>'});
        api.seedArtifact('Run tracker', {'index.html': '<p>r</p>'});
        await tester.pumpWidget(
          app(
            Launcher(
              api: api,
              open: (_) {},
              make: (r, cancelled, stage) => throw StateError('unused'),
              voice: FakeVoice.unsupported('No speech here'),
              makes: makes,
            ),
          ),
        );
        for (var i = 0; i < 10; i++) {
          await tester.pump(const Duration(milliseconds: 100));
        }
        await expectLater(
          find.byType(MaterialApp),
          matchesGoldenFile('$_out/launcher-waiting-$scheme.png'),
        );
      },
    );

    testWidgets('thread waiting for the connection, $scheme', skip: !_on, (
      tester,
    ) async {
      await phone(tester);
      final api = FakeClaxApi(clock: () => '2026-10-15T09:14:00Z');
      final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
      await api.registerAppSession(installId: 'install', pid: 1);
      api.sendComment(aid, 'Show the weekly total under the list');
      final batch = await api.pollFeedback(api.sessionId!, wait: Duration.zero);
      await api.ackFeedback(api.sessionId!, [
        for (final i in batch.items) i.commentId,
      ]);
      final tid = batch.items.single.threadId;
      await tester.pumpWidget(
        app(
          ThreadDetail(
            api: api,
            aid: aid,
            tid: tid,
            voice: FakeVoice(),
            autoDictate: false,
            onPreview: (_) {},
            onRetry: null,
            now: () => DateTime.utc(2026, 10, 15, 9, 20),
            waitingForConnection: ValueNotifier([tid]),
            onCancel: () {},
          ),
        ),
      );
      await tester.pumpAndSettle();
      await expectLater(
        find.byType(MaterialApp),
        matchesGoldenFile('$_out/thread-waiting-$scheme.png'),
      );
    });
  }
}
