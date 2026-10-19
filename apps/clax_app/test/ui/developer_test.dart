import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_inspector.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

class _Key implements SecureKey {
  String? value;

  @override
  Future<String?> read() async => value;

  @override
  Future<void> write(String? key) async => value = key;
}

/// The settings list's own scrollable, not a text field's.
final _list = find
    .descendant(of: find.byType(CustomScrollView), matching: find.byType(Scrollable))
    .first;

Future<void> _tab(WidgetTester tester, String name) async {
  await tester.ensureVisible(find.widgetWithText(Tab, name));
  await tester.pumpAndSettle();
  await tester.tap(find.widgetWithText(Tab, name));
  await tester.pumpAndSettle();
}

void main() {
  group('DeveloperScope', () {
    testWidgets('is off without a scope', (tester) async {
      late bool on;
      await tester.pumpWidget(
        Builder(
          builder: (context) {
            on = DeveloperScope.of(context);
            return const SizedBox();
          },
        ),
      );
      expect(on, isFalse);
    });

    testWidgets('follows its notifier and rebuilds its dependents', (
      tester,
    ) async {
      final developer = ValueNotifier(false);
      addTearDown(developer.dispose);
      var builds = 0;
      late bool on;
      await tester.pumpWidget(
        DeveloperScope(
          notifier: developer,
          child: Builder(
            builder: (context) {
              builds++;
              on = DeveloperScope.of(context);
              return const SizedBox();
            },
          ),
        ),
      );
      expect(on, isFalse);
      developer.value = true;
      await tester.pump();
      expect(on, isTrue);
      expect(builds, 2);
    });
  });

  group('Settings', () {
    late Directory dir;
    late File file;

    setUp(() {
      dir = Directory.systemTemp.createTempSync('clax-developer-');
      file = File('${dir.path}/settings.json');
    });
    tearDown(() => dir.deleteSync(recursive: true));

    testWidgets('shows Recent timings only once Developer tools is on, every '
        'name labelled', (tester) async {
      tester.view.physicalSize = const Size(400, 2400);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final store = SettingsStore(file);
      final developer = ValueNotifier(false);
      addTearDown(developer.dispose);
      await tester.pumpWidget(
        DeveloperScope(
          notifier: developer,
          child: MaterialApp(
            home: SettingsScreen(
              store: store,
              secureKey: _Key(),
              cliSupported: false,
              timings: [
                TimingRecord(
                  name: sendFirstStep,
                  elapsed: const Duration(milliseconds: 1200),
                  at: DateTime.utc(2026, 10, 12, 9),
                ),
              ],
            ),
          ),
        ),
      );
      await tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await tester.pumpAndSettle();

      await tester.scrollUntilVisible(
        find.text('Developer tools'),
        100,
        scrollable: _list,
      );
      expect(find.text('Recent timings'), findsNothing);
      expect(find.text('1.2 s'), findsNothing);

      await tester.tap(find.text('Developer tools'));
      await tester.pumpAndSettle();
      await tester.scrollUntilVisible(
        find.text('1.2 s'),
        100,
        scrollable: _list,
      );
      expect(find.text('Recent timings'), findsOneWidget);
      expect(find.text('Send → first step'), findsOneWidget);
      expect(find.text('send-first-step'), findsNothing);
      expect(store.last.developer, isTrue);
      expect(developer.value, isTrue, reason: 'the app sees it at once');

      await tester.tap(find.text('Developer tools'));
      await tester.pumpAndSettle();
      expect(find.text('Recent timings'), findsNothing);
      expect(store.last.developer, isFalse);
      expect(developer.value, isFalse);
    });

    test('every timing name has its own label', () {
      for (final name in [
        sayUsable,
        longpressLanded,
        homeUsable,
        homeCover,
        sendFirstStep,
      ]) {
        expect(timingLabel(name), isNot(name));
      }
    });
  });

  group('the inspector', () {
    late FakeClaxApi api;
    late String aid;

    setUp(() {
      api = FakeClaxApi();
      aid = api.seedArtifact(
        'Runs',
        {'index.html': '<!doctype html>\n<title>Runs</title>\n'},
        capabilities: {
          'db': {'read': <Object?>[]},
          'notify': <String, Object?>{},
        },
      );
    });

    Future<void> pump(WidgetTester tester, {required bool developer}) async {
      tester.view.physicalSize = const Size(400, 1400) * 2;
      tester.view.devicePixelRatio = 2;
      addTearDown(tester.view.reset);
      final on = ValueNotifier(developer);
      addTearDown(on.dispose);
      await tester.pumpWidget(
        DeveloperScope(
          notifier: on,
          child: MaterialApp(
            theme: claxTheme(Brightness.light, defaultTargetPlatform),
            home: AppInspector(api: api, aid: aid),
          ),
        ),
      );
      await tester.pumpAndSettle();
    }

    for (final developer in [false, true]) {
      final shown = developer ? findsOneWidget : findsNothing;
      final mode = developer ? 'on' : 'off';

      testWidgets('capability names show only with Developer on ($mode)', (
        tester,
      ) async {
        await pump(tester, developer: developer);
        expect(find.text('Capabilities'), shown);
        expect(find.text('db, notify'), shown);
      });

      testWidgets('MIME types show only with Developer on ($mode)', (
        tester,
      ) async {
        await pump(tester, developer: developer);
        await _tab(tester, 'Files');
        expect(find.text('2 lines'), developer ? findsNothing : findsOneWidget);
        expect(find.text('2 lines · text/html; charset=utf-8'), shown);
      });

      testWidgets('the Coming rows show only with Developer on ($mode)', (
        tester,
      ) async {
        await pump(tester, developer: developer);
        await _tab(tester, 'Permissions');
        expect(
          find.text('Coming'),
          developer ? findsNWidgets(3) : findsNothing,
        );
        for (final t in ['Device', 'Network', 'AI']) {
          expect(find.text(t), shown);
        }
      });
    }
  });
}
