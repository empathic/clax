// Renders a thread whose change stopped at its limit twice: the reply with
// how often it stopped, what it cost, the files its draft changed and the
// agent's latest notes, and Keep going, on a phone, light and dark, for a
// person to look at. Off by default; run with
//   CLAX_KG_SHOTS=1 flutter test --update-goldens test/ui/keep_going_shots_test.dart
// and look at target/flutter-app/kg/*.png.
import 'dart:io';

import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/thread_detail.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

final _on = Platform.environment['CLAX_KG_SHOTS'] == '1';
// Goldens resolve from this file.
const _out = '../../../../target/flutter-app/kg';

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
    testWidgets('a limit stop offering Keep going, $scheme', skip: !_on, (
      tester,
    ) async {
      await tester.runAsync(_fonts);
      tester.view.physicalSize = const Size(412 * 2, 915 * 2);
      tester.view.devicePixelRatio = 2;
      addTearDown(tester.view.reset);
      final api = FakeClaxApi(clock: () => '2026-10-16T09:14:00Z');
      final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
      await api.registerAppSession(installId: 'install', pid: 1);
      final item = api.sendComment(
        aid,
        'Add a weekly chart with filters by run type',
        queue: false,
      );
      await api.replyAsAgent(
        aid,
        item.threadId,
        limitFailureReply(
          '10 turns',
          leftUsd: changeCapUsd - 2.1,
          progress: const LimitProgress(
            stops: 2,
            spentUsd: 2.1,
            notes: [
              'Adding the chart canvas to the stats screen.',
              'Wiring the run-type filter to the chart.',
              'Fixing the failing week-boundary test.',
            ],
            changed: [
              (path: 'index.html', added: 42, removed: 10),
              (path: 'app.js', added: 8, removed: 0),
              (path: 'tests/week.test.js', added: 21, removed: 0),
            ],
          ),
        ),
      );
      await tester.pumpWidget(
        MaterialApp(
          debugShowCheckedModeBanner: false,
          theme: claxTheme(Brightness.light, defaultTargetPlatform),
          darkTheme: claxTheme(Brightness.dark, defaultTargetPlatform),
          themeMode: dark ? ThemeMode.dark : ThemeMode.light,
          home: ThreadDetail(
            api: api,
            aid: aid,
            tid: item.threadId,
            voice: FakeVoice(),
            autoDictate: false,
            onPreview: (_) {},
            onRetry: (_) async => true,
            now: () => DateTime.utc(2026, 10, 16, 9, 20),
          ),
        ),
      );
      await tester.pumpAndSettle();
      expect(find.byKey(const ValueKey('thread-keep-going')), findsOneWidget);
      await expectLater(
        find.byType(MaterialApp),
        matchesGoldenFile('$_out/limit-stop-$scheme.png'),
      );
    });
  }
}
