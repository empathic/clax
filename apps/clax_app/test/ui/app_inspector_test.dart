import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/app_icon.dart' show cleanIconsHere;
import 'package:clax_app/ui/app_inspector.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/ui/app_inspector_code.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/kit/mark.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/read_consent_dialog.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

const _files = {
  'index.html':
      '<!doctype html>\n<title>Runs</title>\n<script src="app.js"></script>\n',
  'app.js': '// the app\nconst total = 42;\nexport function sum(a, b) {\n  return a + b;\n}\n',
  'kit/kit.css': ':root { --gap: 12px; }\n',
  'kit/preact.js': 'export const h = () => null;\n',
};

/// An app "Runs" with files, 3 runs and a setting; "Dashboard" granted to
/// read it, and "Runs" granted to read "Weather".
({FakeClaxApi api, String runs, String dash, String weather}) _seed() {
  final api = FakeClaxApi();
  final runs = api.seedArtifact(
    'Runs',
    _files,
    capabilities: {
      'db': {'read': <Object?>[]},
    },
  );
  final dash = api.seedArtifact('Dashboard', {'index.html': '<p>d</p>'});
  final weather = api.seedArtifact('Weather', {'index.html': '<p>w</p>'});
  for (var i = 1; i <= 3; i++) {
    api.putDoc(runs, 'runs/r$i', {'km': i * 5, 'note': 'run $i'});
  }
  api.putDoc(runs, 'settings/main', {'unit': 'km'});
  api.grantRead(dash, runs);
  api.grantRead(runs, weather);
  return (api: api, runs: runs, dash: dash, weather: weather);
}

/// Pumps [child] in the app's theme, with Developer tools on when
/// [developer].
Future<void> _pump(
  WidgetTester tester,
  Widget child, {
  Size size = const Size(400, 860),
  bool developer = false,
}) async {
  tester.view.physicalSize = size * 2;
  tester.view.devicePixelRatio = 2;
  addTearDown(tester.view.reset);
  final on = ValueNotifier(developer);
  addTearDown(on.dispose);
  await tester.pumpWidget(
    DeveloperScope(
      notifier: on,
      child: MaterialApp(
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
        home: child,
      ),
    ),
  );
  await tester.pumpAndSettle();
}

List<String> _tabs(WidgetTester tester) => [
  for (final t in tester.widgetList<Tab>(find.byType(Tab))) t.text!,
];

const _boom = ClaxApiError(status: 500, code: 'internal', message: 'boom');

Future<void> _tab(WidgetTester tester, String name) async {
  await tester.ensureVisible(find.widgetWithText(Tab, name));
  await tester.pumpAndSettle();
  await tester.tap(find.widgetWithText(Tab, name));
  await tester.pumpAndSettle();
}

void main() {
  testWidgets('four plain tabs, and Files, Code and Data with Developer on', (
    tester,
  ) async {
    final s = _seed();
    await _pump(tester, AppInspector(api: s.api, aid: s.runs));
    expect(_tabs(tester), ['Overview', 'Versions', 'Permissions', 'Plan']);
    await _pump(
      tester,
      AppInspector(key: const ValueKey('dev'), api: s.api, aid: s.runs),
      developer: true,
    );
    expect(_tabs(tester), [
      'Overview',
      'Versions',
      'Permissions',
      'Plan',
      'Files',
      'Code',
      'Data',
    ]);
  });

  testWidgets('the bar says Inspect and the hero names the app once', (
    tester,
  ) async {
    final s = _seed();
    await _pump(tester, AppInspector(api: s.api, aid: s.runs));
    expect(
      find.descendant(of: find.byType(AppBar), matching: find.text('Inspect')),
      findsOneWidget,
    );
    expect(find.text('Runs'), findsOneWidget);
    expect(find.text('Inspector'), findsNothing);
  });

  testWidgets('the plain tabs read no file texts', (tester) async {
    final s = _seed();
    await _pump(tester, AppInspector(api: s.api, aid: s.runs));
    for (final t in ['Versions', 'Permissions', 'Plan', 'Overview']) {
      await _tab(tester, t);
    }
    expect(s.api.callNames, isNot(contains('readFiles')));
  });

  testWidgets('a failed read of the files shows Try again in Files and Code '
      'alone, while Overview renders', (tester) async {
    final s = _seed();
    s.api.failNext('readFiles', _boom);
    await _pump(
      tester,
      AppInspector(api: s.api, aid: s.runs),
      developer: true,
    );
    expect(find.text(s.runs), findsOneWidget, reason: 'Overview renders');
    expect(find.text('Try again'), findsNothing);
    await _tab(tester, 'Files');
    expect(find.text('Try again'), findsOneWidget);
    await _tab(tester, 'Data');
    expect(find.text('runs'), findsOneWidget);
    expect(find.text('Try again'), findsNothing);
    await _tab(tester, 'Files');
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(find.text('app.js'), findsOneWidget);
    expect(find.text('Try again'), findsNothing);
  });

  testWidgets('a failed read of the data shows Try again in Data alone', (
    tester,
  ) async {
    final s = _seed();
    s.api.failNext('collections', _boom);
    await _pump(
      tester,
      AppInspector(api: s.api, aid: s.runs),
      developer: true,
    );
    expect(find.text(s.runs), findsOneWidget, reason: 'Overview renders');
    await _tab(tester, 'Files');
    expect(find.text('app.js'), findsOneWidget);
    await _tab(tester, 'Data');
    expect(find.text('Try again'), findsOneWidget);
    await tester.tap(find.text('Try again'));
    await tester.pumpAndSettle();
    expect(find.text('runs'), findsOneWidget);
  });

  testWidgets('Overview says what the app does in plain words', (
    tester,
  ) async {
    final api = FakeClaxApi();
    final aid = api.seedArtifact(
      'Weather',
      {'index.html': '<p>w</p>'},
      capabilities: {
        'db': <String, Object?>{},
        'net': {
          'hosts': ['api.weather.example'],
        },
        'notify': <String, Object?>{},
      },
    );
    await _pump(tester, AppInspector(api: api, aid: aid));
    expect(find.text('Saves its own data'), findsOneWidget);
    expect(find.text('Uses the internet: api.weather.example'), findsOneWidget);
    expect(find.text('Sends notifications'), findsOneWidget);
    expect(find.textContaining('db, net'), findsNothing);
    expect(
      capabilityLines({
        'net': <String, Object?>{},
        'sample': <String, Object?>{},
        'comments': <String, Object?>{},
      }),
      ['Uses the internet', 'Uses AI'],
    );
  });

  testWidgets('Overview shows the facts and copies the ID', (tester) async {
    final s = _seed();
    String? copied;
    tester.binding.defaultBinaryMessenger.setMockMethodCallHandler(
      SystemChannels.platform,
      (call) async {
        if (call.method == 'Clipboard.setData') {
          copied = (call.arguments as Map)['text'] as String;
        }
        return null;
      },
    );
    await _pump(tester, AppInspector(api: s.api, aid: s.runs));
    expect(find.text('Runs'), findsOneWidget);
    expect(find.text(s.runs), findsOneWidget);
    final body = find.byType(TabBarView);
    expect(
      find.descendant(of: body, matching: find.text('Versions')),
      findsOneWidget,
    );
    expect(find.text('Documents'), findsOneWidget);
    expect(find.text('4'), findsOneWidget, reason: 'four documents');
    expect(find.textContaining('4 documents in 2 collections'), findsOneWidget);
    final size = _files.values.fold<int>(0, (n, t) => n + t.length);
    expect(find.textContaining('4 · '), findsOneWidget);
    expect(find.textContaining('$size B'), findsWidgets);
    await tester.tap(find.byKey(const ValueKey('copy-id')));
    await tester.pumpAndSettle();
    expect(copied, s.runs);
  });

  testWidgets('Overview shows the app icon, or the tile face without one', (
    tester,
  ) async {
    final api = FakeClaxApi();
    final withIcon = api.seedArtifact('Runs', {
      'index.html': '<p>r</p>',
      'icon.svg':
          '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">'
          '<rect width="64" height="64" fill="#f80"/></svg>',
    });
    final plain = api.seedArtifact('Plain', {'index.html': '<p>p</p>'});
    await _pump(
      tester,
      AppInspector(api: api, aid: withIcon, cleanIcons: cleanIconsHere),
    );
    expect(find.byKey(const ValueKey('app-icon')), findsOneWidget);
    await _pump(
      tester,
      AppInspector(key: ValueKey(plain), api: api, aid: plain),
    );
    expect(find.byKey(const ValueKey('app-icon')), findsNothing);
    expect(find.byKey(const ValueKey('app-face')), findsOneWidget);
  });

  testWidgets('Versions lists the history; a version opens the app there', (
    tester,
  ) async {
    final s = _seed();
    final opened = <(String, int)>[];
    await _pump(
      tester,
      AppInspector(
        api: s.api,
        aid: s.runs,
        onOpenVersion: (aid, n) => opened.add((aid, n)),
      ),
    );
    await _tab(tester, 'Versions');
    expect(find.text('v1'), findsOneWidget);
    expect(find.byKey(const ValueKey('restore-1')), findsNothing);
    await tester.tap(find.byKey(const ValueKey('version-1')));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Preview'));
    await tester.pumpAndSettle();
    expect(opened, [(s.runs, 1)]);
  });

  testWidgets('Files lists the tree with sizes and opens a file read-only', (
    tester,
  ) async {
    final s = _seed();
    await _pump(
      tester,
      AppInspector(api: s.api, aid: s.runs),
      developer: true,
    );
    await _tab(tester, 'Files');
    expect(find.text('kit/'), findsOneWidget);
    expect(find.text('index.html'), findsOneWidget);
    expect(find.text('app.js'), findsOneWidget);
    expect(find.text('kit.css'), findsNothing, reason: 'folders start closed');
    await tester.tap(find.text('kit/'));
    await tester.pumpAndSettle();
    expect(find.text('kit.css'), findsOneWidget);

    await tester.tap(find.text('app.js'));
    await tester.pumpAndSettle();
    expect(find.byType(CodeView), findsOneWidget);
    expect(find.text('5'), findsOneWidget, reason: 'line numbers');
    expect(
      find.byWidgetPredicate(
        (w) =>
            w is RichText && w.text.toPlainText().contains('const total = 42;'),
      ),
      findsOneWidget,
    );
    expect(find.byType(TextField), findsNothing, reason: 'read-only');
  });

  testWidgets('Code shows the app\u2019s text files, one at a time', (
    tester,
  ) async {
    final s = _seed();
    await _pump(
      tester,
      AppInspector(api: s.api, aid: s.runs),
      developer: true,
    );
    await _tab(tester, 'Code');
    bool shows(String text) => find
        .byWidgetPredicate(
          (w) => w is RichText && w.text.toPlainText().contains(text),
        )
        .evaluate()
        .isNotEmpty;
    expect(find.byType(CodeView), findsOneWidget);
    expect(shows('<title>Runs</title>'), isTrue, reason: 'index.html first');
    await tester.tap(find.byKey(const ValueKey('code-app.js')));
    await tester.pumpAndSettle();
    expect(shows('const total = 42;'), isTrue);
    expect(shows('<title>Runs</title>'), isFalse);
  });

  testWidgets('Data lists collections and shows documents as key/value '
      'tables, the raw JSON behind a toggle', (tester) async {
    final s = _seed();
    await _pump(
      tester,
      AppInspector(api: s.api, aid: s.runs),
      developer: true,
    );
    await _tab(tester, 'Data');
    expect(find.text('runs'), findsOneWidget);
    expect(find.text('3'), findsOneWidget);
    expect(find.text('settings'), findsOneWidget);

    await tester.tap(find.text('runs'));
    await tester.pumpAndSettle();
    // The default page holds all three.
    expect(find.text('r1'), findsOneWidget);
    expect(find.text('r3'), findsOneWidget);
    expect(find.byKey(const ValueKey('load-more')), findsNothing);
    final r2 = find.byKey(const ValueKey('doc-r2'));
    expect(
      find.descendant(of: r2, matching: find.text('km')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: r2, matching: find.text('10')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: r2, matching: find.text('run 2')),
      findsOneWidget,
    );
    expect(find.textContaining('"km"'), findsNothing, reason: 'no JSON yet');
    await tester.tap(find.byKey(const ValueKey('raw-json')));
    await tester.pumpAndSettle();
    expect(find.textContaining('"km": 10'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('raw-json')));
    await tester.pumpAndSettle();
    expect(find.textContaining('"km"'), findsNothing);

    await tester.tap(find.text('r2'));
    await tester.pumpAndSettle();
    expect(find.byType(CodeView), findsNothing);
    expect(find.text('run 2'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('raw-json')));
    await tester.pumpAndSettle();
    expect(find.byType(CodeView), findsOneWidget);
    expect(
      find.byWidgetPredicate(
        (w) => w is RichText && w.text.toPlainText().contains('"km": 10'),
      ),
      findsOneWidget,
    );
  });

  testWidgets('a large collection loads a page at a time', (tester) async {
    final s = _seed();
    await _pump(
      tester,
      CollectionPage(
        api: s.api,
        aid: s.runs,
        collection: 'runs',
        count: 3,
        pageSize: 2,
      ),
    );
    expect(find.text('r1'), findsOneWidget);
    expect(find.text('r2'), findsOneWidget);
    expect(find.text('r3'), findsNothing);
    await tester.tap(find.byKey(const ValueKey('load-more')));
    await tester.pumpAndSettle();
    expect(find.text('r3'), findsOneWidget);
    expect(find.byKey(const ValueKey('load-more')), findsNothing);
    final pages = s.api.calls.where((c) => c.name == 'pageDocs').toList();
    expect(pages.map((c) => c.args['cursor']), [null, '2']);
  });

  testWidgets('Permissions lists grants both ways and Clax\u2019s consents, '
      'revokes one, and hides the coming kinds without Developer', (
    tester,
  ) async {
    final s = _seed();
    final memory = MemoryConsentMemory();
    await memory.allow(s.runs, s.weather);
    await _pump(
      tester,
      AppInspector(
        api: s.api,
        aid: s.runs,
        permissions: [AgentReadPermissions(memory)],
      ),
      size: const Size(400, 1400),
    );
    await _tab(tester, 'Permissions');
    expect(find.text('Clax reads'), findsOneWidget);
    expect(find.text('Agent reads'), findsNothing);
    expect(find.byIcon(Icons.smart_toy_outlined), findsNothing);
    expect(
      find.ancestor(
        of: find.byType(ClaxMark),
        matching: find.byKey(const ValueKey('section-agent')),
      ),
      findsOneWidget,
    );
    expect(find.text('Reads Weather'), findsOneWidget);
    expect(find.text('Dashboard reads this app'), findsOneWidget);
    expect(find.text('Reads Weather while building this app'), findsOneWidget);
    for (final t in ['Device', 'Network', 'AI']) {
      expect(find.text(t), findsNothing);
    }
    expect(find.text('Coming'), findsNothing);

    final row = find.ancestor(
      of: find.text('Dashboard reads this app'),
      matching: find.byType(ListTile),
    );
    await tester.tap(find.descendant(of: row, matching: find.text('Revoke')));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Revoke'));
    await tester.pumpAndSettle();
    expect(s.api.calls.where((c) => c.name == 'revokeRead').single.args, {
      'reader': s.dash,
      'source': s.runs,
    });
    expect(find.text("Dashboard reads this app"), findsNothing);
    expect(find.text('Reads Weather'), findsOneWidget);

    final agent = find.ancestor(
      of: find.text('Reads Weather while building this app'),
      matching: find.byType(ListTile),
    );
    await tester.tap(find.descendant(of: agent, matching: find.text('Revoke')));
    await tester.pumpAndSettle();
    await tester.tap(find.widgetWithText(FilledButton, 'Revoke'));
    await tester.pumpAndSettle();
    expect(await memory.pairs(), isEmpty);
    expect(find.text('Reads Weather while building this app'), findsNothing);
  });

  testWidgets('Plan says No plan without one, and shows the given one', (
    tester,
  ) async {
    final s = _seed();
    await _pump(tester, AppInspector(api: s.api, aid: s.runs));
    await _tab(tester, 'Plan');
    expect(find.text('No plan'), findsOneWidget);

    await _pump(
      tester,
      AppInspector(
        api: s.api,
        aid: s.runs,
        plan: (context, aid) => Text('plan of $aid'),
      ),
    );
    await _tab(tester, 'Plan');
    expect(find.text('plan of ${s.runs}'), findsOneWidget);
  });

  testWidgets('a long press on a launcher tile offers Inspect', (tester) async {
    final s = _seed();
    await _pump(
      tester,
      Launcher(
        api: s.api,
        open: (_) {},
        make: (r, c, st) => throw StateError('unused'),
        voice: FakeVoice.unsupported('none'),
      ),
    );
    await tester.longPress(find.text('Runs'));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('inspect')));
    await tester.pumpAndSettle();
    expect(find.byType(AppInspector), findsOneWidget);
    expect(find.text('Inspect'), findsWidgets);
    expect(find.text(s.runs), findsOneWidget);
  });

  test('highlight marks comments, strings, numbers and keywords', () {
    final js = highlight(['/* a', 'b */ const x = "s"; // c'], CodeLanguage.js);
    expect(js[0].single.kind, TokenKind.comment);
    expect(js[1].first, (text: 'b */', kind: TokenKind.comment));
    final kinds = {for (final t in js[1]) t.text: t.kind};
    expect(kinds['const'], TokenKind.keyword);
    expect(kinds['"s"'], TokenKind.string);
    expect(kinds['// c'], TokenKind.comment);
    final template = highlight(['html`<p>', '  <//>`;'], CodeLanguage.js);
    expect(template[1].first, (text: '  <//>`', kind: TokenKind.string));
    final json = highlight(['{"a": 1, "b": true}'], CodeLanguage.json).single;
    expect({
      for (final t in json) t.text: t.kind,
    }, containsPair('1', TokenKind.number));
    expect(
      highlight(
        ['<p class="x">hi</p>'],
        CodeLanguage.html,
      ).single.where((t) => t.kind == TokenKind.tag).map((t) => t.text),
      ['<p', '>', '</p', '>'],
    );
  });
}
