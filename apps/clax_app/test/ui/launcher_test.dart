import 'dart:async';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

const _page = {'index.html': '<!doctype html><title>x</title>'};

/// A fake whose stamps increase, so later artifacts are newer.
FakeClaxApi _api() {
  var t = 0;
  return FakeClaxApi(
    clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
  );
}

void main() {
  late List<String> opened;

  setUp(() => opened = []);

  Future<void> pumpLauncher(
    WidgetTester tester,
    ClaxApi api, {
    Future<String> Function(String)? make,
    FakeVoice? voice,
    VoidCallback? openSettings,
  }) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Launcher(
          api: api,
          open: opened.add,
          make: make ?? (_) async => throw StateError('unused'),
          voice: voice ?? FakeVoice.unsupported('No speech here'),
          openSettings: openSettings,
        ),
      ),
    );
    await tester.pumpAndSettle();
  }

  testWidgets('shows the apps newest first', (tester) async {
    final api = _api();
    api.seedArtifact('Old runs', _page);
    api.seedArtifact('Groceries', _page);
    await pumpLauncher(tester, api);
    final older = tester.getTopLeft(find.text('Old runs'));
    final newer = tester.getTopLeft(find.text('Groceries'));
    expect(
      newer.dy < older.dy || (newer.dy == older.dy && newer.dx < older.dx),
      isTrue,
      reason: 'the newest app comes first',
    );
  });

  testWidgets('an empty launcher invites the first app', (tester) async {
    await pumpLauncher(tester, _api());
    expect(find.text('Say what you want, and Clax makes it.'), findsOneWidget);
    expect(find.text('New app'), findsOneWidget);
  });

  testWidgets('a version event for a new artifact adds its tile', (
    tester,
  ) async {
    final api = _api();
    api.seedArtifact('Old runs', _page);
    await pumpLauncher(tester, api);
    final aid = api.seedArtifact('Fresh app', _page);
    expect(find.text('Fresh app'), findsNothing);
    api.emit(ClaxEvent(type: 'version', data: {'artifact_id': aid, 'n': 1}));
    await tester.pumpAndSettle();
    expect(find.text('Fresh app'), findsOneWidget);
    expect(
      api.calls.where((c) => c.name == 'events').single.args['types'],
      containsAll(['version', 'artifact_deleted']),
    );
  });

  testWidgets('an artifact_deleted event removes its tile', (tester) async {
    final api = _api();
    final aid = api.seedArtifact('Doomed', _page);
    await pumpLauncher(tester, api);
    expect(find.text('Doomed'), findsOneWidget);
    api.emit(ClaxEvent(type: 'artifact_deleted', data: {'artifact_id': aid}));
    await tester.pumpAndSettle();
    expect(find.text('Doomed'), findsNothing);
  });

  testWidgets('tapping a tile opens its app', (tester) async {
    final api = _api();
    final aid = api.seedArtifact('Runs', _page);
    await pumpLauncher(tester, api);
    await tester.tap(find.text('Runs'));
    expect(opened, [aid]);
  });

  testWidgets('New, typing and submit makes the app and opens it', (
    tester,
  ) async {
    final api = _api();
    final asked = <String>[];
    await pumpLauncher(
      tester,
      api,
      make: (r) async {
        asked.add(r);
        return 'made123';
      },
    );
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    expect(find.text('Say or type what you want'), findsOneWidget);
    await tester.enterText(find.byType(TextField), 'a tracker for my runs');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pumpAndSettle();
    expect(asked, ['a tracker for my runs']);
    expect(opened, ['made123']);
    expect(find.textContaining('Making'), findsNothing);
  });

  testWidgets('making shows the request, the seconds, and Cancel', (
    tester,
  ) async {
    final pending = Completer<String>();
    await pumpLauncher(tester, _api(), make: (_) => pending.future);
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(
      find.byType(TextField),
      'a tracker for my runs with a weekly total',
    );
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('Making “a tracker for my runs”…'), findsOneWidget);
    expect(find.text('0 s'), findsOneWidget);
    await tester.pump(const Duration(seconds: 3));
    expect(find.text('3 s'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await tester.pumpAndSettle();
    expect(find.textContaining('Making'), findsNothing);
    // The abandoned run finishing later opens nothing.
    pending.complete('late');
    await tester.pumpAndSettle();
    expect(opened, isEmpty);
    // The request is kept for the next New.
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    expect(
      find.text('a tracker for my runs with a weekly total'),
      findsOneWidget,
    );
  });

  testWidgets('a failed make shows the reason and Retry, keeping the text', (
    tester,
  ) async {
    var attempts = 0;
    await pumpLauncher(
      tester,
      _api(),
      make: (_) async {
        if (++attempts == 1) throw const AgentError('The model is busy');
        return 'second';
      },
    );
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'a habit tracker');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pumpAndSettle();
    expect(find.text('The model is busy'), findsOneWidget);
    expect(find.text('a habit tracker'), findsOneWidget);
    expect(opened, isEmpty);
    await tester.tap(find.text('Retry'));
    await tester.pumpAndSettle();
    expect(attempts, 2);
    expect(opened, ['second']);
  });

  testWidgets('a failure naming Settings links there', (tester) async {
    var settings = 0;
    await pumpLauncher(
      tester,
      _api(),
      make: (_) async =>
          throw const AgentError('Add your Claude API key in Settings'),
      openSettings: () => settings++,
    );
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), 'a timer');
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pumpAndSettle();
    expect(find.text('Add your Claude API key in Settings'), findsOneWidget);
    await tester.tap(find.text('Open Settings'));
    await tester.pumpAndSettle();
    expect(settings, 1);
  });

  testWidgets('the settings button shows only when it has somewhere to go', (
    tester,
  ) async {
    await pumpLauncher(tester, _api());
    expect(find.byTooltip('Settings'), findsNothing);
    var settings = 0;
    await pumpLauncher(tester, _api(), openSettings: () => settings++);
    await tester.tap(find.byTooltip('Settings'));
    expect(settings, 1);
  });

  testWidgets('the launcher follows the dark theme', (tester) async {
    final api = _api();
    api.seedArtifact('Runs', _page);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light),
        darkTheme: claxTheme(Brightness.dark),
        themeMode: ThemeMode.dark,
        home: Launcher(
          api: api,
          open: opened.add,
          make: (_) async => 'x',
          voice: FakeVoice.unsupported('No speech here'),
        ),
      ),
    );
    await tester.pumpAndSettle();
    final scaffold = tester.widget<Scaffold>(find.byType(Scaffold));
    final bg =
        scaffold.backgroundColor ??
        Theme.of(tester.element(find.byType(Scaffold))).scaffoldBackgroundColor;
    expect(bg.computeLuminance(), lessThan(0.1));
    expect(
      Theme.of(tester.element(find.text('Runs'))).brightness,
      Brightness.dark,
    );
  });
}
