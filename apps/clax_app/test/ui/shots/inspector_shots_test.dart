// The app inspector and the version page, rendered in every form and theme
// into CLAX_SHOTS_DIR (skipped without it): the inspector's Overview,
// Versions, Permissions and Plan with Developer tools off; its Files, Code
// and Data tabs, a collection and a document with them on; and a version's
// page with its changed files and one file's diff.
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/app_inspector.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/read_consent_dialog.dart';
import 'package:clax_app/ui/version_page.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

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

const _icon =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">'
    '<rect width="64" height="64" rx="14" fill="#2f7d5b"/>'
    '<path d="M18 44 L30 20 L38 34 L46 26" stroke="#fff" stroke-width="5" '
    'fill="none" stroke-linecap="round" stroke-linejoin="round"/></svg>';

const _index =
    '<!doctype html>\n<html lang="en">\n<head>\n  <meta charset="utf-8">\n'
    '  <title>Run log</title>\n  <link rel="stylesheet" href="kit/kit.css">\n'
    '</head>\n<body>\n  <!-- the app renders here -->\n'
    '  <script type="module" src="app.js"></script>\n</body>\n</html>\n';

Future<({FakeClaxApi api, String runs, ConsentMemory memory})> _seed() async {
  final api = FakeClaxApi(clock: () => '2026-10-12T08:30:00Z');
  final runs = api.seedArtifact(
    'Run log',
    {
      'index.html': _index,
      'app.js': _appJs,
      'icon.svg': _icon,
      'tests/total.test.js': 'test("sums the week", () => {});\n',
      'kit/kit.css': ':root {\n  --gap: 12px;\n  --radius: 16px;\n}\n' * 40,
      'kit/kit.js': 'export const App = () => null;\n' * 120,
      'kit/preact.js': 'export const h = () => null;\n' * 300,
    },
    capabilities: {
      'db': {'read': <Object?>[]},
      'net': {
        'hosts': ['api.weather.example'],
      },
      'notify': <String, Object?>{},
    },
  );
  final dash = api.seedArtifact('Weekly dashboard', {'index.html': '<p>d</p>'});
  final weather = api.seedArtifact('Weather', {'index.html': '<p>w</p>'});
  for (var i = 1; i <= 12; i++) {
    final day = '2026-10-${i.toString().padLeft(2, '0')}';
    api.putDoc(runs, 'runs/$day', {
      'at': '${day}T07:00:00Z',
      'km': 4 + i * 0.5,
      'minutes': 24 + i * 2,
      'route': i.isEven ? 'River loop' : 'Park hills',
      'felt': ['easy', 'tempo'],
    });
  }
  api.putDoc(runs, 'settings/main', {'unit': 'km', 'goal': 30});
  api.grantRead(dash, runs);
  api.grantRead(runs, weather);
  await api.registerAppSession(installId: 'i', pid: 1);
  await api.publish(
    runs,
    PublishRequest(
      ifVersion: 1,
      note: 'Show this week’s total above the runs',
      files: {
        'index.html': _index,
        'app.js': _appJs.replaceFirst(
          'This week:',
          'This week, so far:',
        ),
        'kit/kit.css': ':root {\n  --gap: 14px;\n  --radius: 16px;\n}\n' * 40,
      },
    ),
  );
  await api.publish(
    runs,
    PublishRequest(
      ifVersion: 2,
      files: {'index.html': _index.replaceFirst('Run log', 'Run log!')},
    ),
  );
  final memory = MemoryConsentMemory();
  await memory.allow(runs, weather);
  return (api: api, runs: runs, memory: memory);
}

Widget _scoped(bool developer, Widget child) {
  final on = ValueNotifier(developer);
  addTearDown(on.dispose);
  return DeveloperScope(notifier: on, child: child);
}

Future<void> _tab(Shot s, String name) async {
  await s.tester.ensureVisible(find.widgetWithText(Tab, name));
  await s.settle(2);
  await s.tester.tap(find.widgetWithText(Tab, name));
  await s.settle();
}

void main() {
  for (final tab in ['Overview', 'Versions', 'Permissions', 'Plan']) {
    final surface = 'inspector-${tab.toLowerCase()}';
    shots(surface, (s) async {
      final seed = await s.tester.runAsync(_seed);
      await s.tester.pumpWidget(
        _scoped(
          false,
          s.app(
            AppInspector(
              api: seed!.api,
              aid: seed.runs,
              permissions: [AgentReadPermissions(seed.memory)],
              onOpenVersion: (_, _) {},
            ),
            title: 'Run log',
          ),
        ),
      );
      await s.settle();
      if (tab != 'Overview') await _tab(s, tab);
      await s.take(surface);
    });
  }

  shots('inspector-developer', (s) async {
    final seed = await s.tester.runAsync(_seed);
    await s.tester.pumpWidget(
      _scoped(
        true,
        s.app(
          AppInspector(
            api: seed!.api,
            aid: seed.runs,
            permissions: [AgentReadPermissions(seed.memory)],
          ),
          title: 'Run log',
        ),
      ),
    );
    await s.settle();
    await _tab(s, 'Files');
    await s.tester.tap(find.text('kit/'));
    await s.settle();
    await s.take('inspector-files');
    await _tab(s, 'Code');
    await s.tester.tap(find.byKey(const ValueKey('code-app.js')));
    await s.settle();
    await s.take('inspector-code');
    await _tab(s, 'Data');
    await s.take('inspector-data');
    await s.tester.tap(find.text('runs'));
    await s.settle();
    await s.take('inspector-collection');
    await s.tester.tap(find.byKey(const ValueKey('raw-json')));
    await s.settle();
    await s.take('inspector-collection-raw');
    await s.tester.tap(find.byKey(const ValueKey('raw-json')));
    await s.settle();
    await s.tester.tap(find.text('2026-10-03'));
    await s.settle();
    await s.take('inspector-document');
  });

  for (final n in [2, 3]) {
    shots('version-page-v$n', (s) async {
      final seed = await s.tester.runAsync(_seed);
      await s.tester.pumpWidget(
        s.app(
          VersionPage(
            api: seed!.api,
            aid: seed.runs,
            n: n,
            onPreview: (_) {},
            onRestore: (_) async {},
          ),
          title: 'Run log',
        ),
      );
      await s.tester.runAsync(() => Future<void>.delayed(Duration.zero));
      await s.settle();
      await s.take('version-page-v$n');
      if (n == 2) {
        await s.tester.tap(find.byKey(const ValueKey('file-app.js')));
        await s.tester.runAsync(() => Future<void>.delayed(Duration.zero));
        await s.settle();
        await s.take('version-diff');
      }
    });
  }
}
