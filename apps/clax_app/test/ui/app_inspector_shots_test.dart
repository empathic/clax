// Renders the app inspector's sections at phone size, light and dark, for a
// person to look at. Off by default; run with
//   CLAX_I1_SHOTS=1 flutter test --update-goldens test/ui/app_inspector_shots_test.dart
// and look at target/flutter-app/i1/*.png.
import 'dart:io';

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/app_inspector.dart';
import 'package:clax_app/ui/read_consent_dialog.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

final _on = Platform.environment['CLAX_I1_SHOTS'] == '1';
// Goldens resolve from this file.
const _out = '../../../../target/flutter-app/i1';

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
  await (FontLoader(
    'monospace',
  )..addFont(f('/System/Library/Fonts/Supplemental/Andale Mono.ttf'))).load();
}

const _appJs = '''import {html, render, useState} from './kit/preact.js';
import {App, AppBar, List, ListItem, Fab} from './kit/kit.js';

/* Runs, newest first, with this week's total. */
const WEEK = 7 * 24 * 3600 * 1000;

function Runs({runs}) {
  const total = runs
    .filter((r) => Date.now() - Date.parse(r.at) < WEEK)
    .reduce((km, r) => km + r.km, 0);
  return html`<\${List} title="This week: \${total.toFixed(1)} km">
    \${runs.map((r) => html`<\${ListItem} title=\${r.km + " km"} />`)}
  <//>`;
}

render(html`<\${App}><\${Runs} runs=\${[]} /><//>`, document.body);
''';

({FakeClaxApi api, String runs, ConsentMemory memory}) _seed() {
  final api = FakeClaxApi(clock: () => '2026-10-12T08:30:00Z');
  final runs = api.seedArtifact(
    'Run log',
    {
      'index.html':
          '<!doctype html>\n<html lang="en">\n<head>\n  <meta charset="utf-8">\n'
          '  <title>Run log</title>\n  <link rel="stylesheet" href="kit/kit.css">\n'
          '</head>\n<body>\n  <!-- the app renders here -->\n'
          '  <script type="module" src="app.js"></script>\n</body>\n</html>\n',
      'app.js': _appJs,
      'tests/total.test.js': 'test("sums the week", () => {});\n',
      'kit/kit.css': ':root {\n  --gap: 12px;\n  --radius: 16px;\n}\n' * 40,
      'kit/kit.js': 'export const App = () => null;\n' * 120,
      'kit/preact.js': 'export const h = () => null;\n' * 300,
    },
    capabilities: {
      'db': {'read': <Object?>[]},
      'notify': <String, Object?>{},
    },
  );
  final dash = api.seedArtifact('Weekly dashboard', {'index.html': '<p>d</p>'});
  final weather = api.seedArtifact('Weather', {'index.html': '<p>w</p>'});
  for (var i = 1; i <= 12; i++) {
    api.putDoc(runs, 'runs/2026-10-${i.toString().padLeft(2, '0')}', {
      'at': '2026-10-${i.toString().padLeft(2, '0')}T07:00:00Z',
      'km': 4 + i * 0.5,
      'minutes': 24 + i * 2,
      'route': i.isEven ? 'River loop' : 'Park hills',
      'felt': ['easy', 'tempo'],
    });
  }
  api.putDoc(runs, 'settings/main', {'unit': 'km', 'goal': 30});
  api.grantRead(dash, runs);
  api.grantRead(runs, weather);
  final memory = MemoryConsentMemory();
  memory.allow(runs, weather);
  return (api: api, runs: runs, memory: memory);
}

void main() {
  for (final dark in [false, true]) {
    final scheme = dark ? 'dark' : 'light';
    testWidgets('app inspector sections, $scheme', skip: !_on, (tester) async {
      await tester.runAsync(_fonts);
      tester.view.physicalSize = const Size(1080, 2340);
      tester.view.devicePixelRatio = 1080 / 412;
      addTearDown(tester.view.reset);
      final s = _seed();
      await tester.pumpWidget(
        MaterialApp(
          debugShowCheckedModeBanner: false,
          theme: claxTheme(Brightness.light),
          darkTheme: claxTheme(Brightness.dark),
          themeMode: dark ? ThemeMode.dark : ThemeMode.light,
          home: AppInspector(
            api: s.api,
            aid: s.runs,
            permissions: [AgentReadPermissions(s.memory)],
            onOpenVersion: (_, _) {},
          ),
        ),
      );
      await tester.pumpAndSettle();
      Future<void> shot(String name) async {
        await tester.pumpAndSettle();
        await expectLater(
          find.byType(MaterialApp),
          matchesGoldenFile('$_out/inspector-$name-$scheme.png'),
        );
      }

      Future<void> tab(String name) async {
        await tester.ensureVisible(find.widgetWithText(Tab, name));
        await tester.pumpAndSettle();
        await tester.tap(find.widgetWithText(Tab, name));
        await tester.pumpAndSettle();
      }

      await shot('overview');
      await tab('Files');
      await tester.tap(find.text('kit/'));
      await shot('files');
      await tester.tap(find.text('app.js'));
      await shot('code');
      await tester.pageBack();
      await tester.pumpAndSettle();
      await tab('Data');
      await shot('data');
      await tester.tap(find.text('runs'));
      await shot('collection');
      await tester.tap(find.text('2026-10-03'));
      await shot('document');
      await tester.pageBack();
      await tester.pumpAndSettle();
      await tester.pageBack();
      await tester.pumpAndSettle();
      await tab('Permissions');
      await shot('permissions');
      await tab('Plan');
      await shot('plan');
    });
  }
}
