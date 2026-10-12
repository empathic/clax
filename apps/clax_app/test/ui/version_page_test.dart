import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/version_page.dart';
import 'package:clax_app/ui/versions_tab.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

String page(List<String> body) =>
    ['<!doctype html>', '<title>Runs</title>', ...body].join('\n');

final v1Body = [for (var i = 1; i <= 20; i++) '<p>row $i</p>'];

class _Rig {
  _Rig(this.api, this.aid);
  final FakeClaxApi api;
  final String aid;
  final previews = <int>[];
  final restores = <int>[];

  Widget versionPage(int n) => MaterialApp(
    theme: claxTheme(Brightness.light),
    home: VersionPage(
      api: api,
      aid: aid,
      n: n,
      onPreview: previews.add,
      onRestore: (n) async => restores.add(n),
    ),
  );
}

/// v1: index.html, app.css, notes.txt. v2: one line of index.html
/// changed, app.css removed, app.js added, notes.txt unchanged. v3: a
/// 4,001-line data.csv added.
Future<_Rig> _rig() async {
  final api = FakeClaxApi(clock: () => '2026-10-12T09:30:00Z');
  final aid = api.seedArtifact('Runs', {
    'index.html': page(v1Body),
    'app.css': 'body { margin: 0 }\n',
    'notes.txt': 'keep\n',
  });
  await api.registerAppSession(installId: 'i', pid: 1);
  final v2Body = [...v1Body]..[9] = '<p>row ten</p>';
  await api.publish(
    aid,
    PublishRequest(
      ifVersion: 1,
      note: 'Name the tenth row',
      label: 'tenth',
      files: {
        'index.html': page(v2Body),
        'app.css': null,
        'app.js': 'console.log(1)\n',
      },
    ),
  );
  await api.publish(
    aid,
    PublishRequest(
      ifVersion: 2,
      note: 'Add the data',
      files: {
        'data.csv': [for (var i = 0; i < 4001; i++) '$i,x'].join('\n'),
      },
    ),
  );
  return _Rig(api, aid);
}

void main() {
  testWidgets('lists the files changed against the version before', (
    tester,
  ) async {
    final r = await _rig();
    await tester.pumpWidget(r.versionPage(2));
    await tester.pumpAndSettle();

    expect(find.text('Version 2'), findsOneWidget);
    expect(find.text('Name the tenth row'), findsOneWidget);
    expect(find.textContaining('tenth'), findsWidgets);
    expect(r.api.callNames, containsAll(['getArtifact', 'readFiles']));
    final reads = [
      for (final c in r.api.calls)
        if (c.name == 'readFiles') c.args['n'],
    ];
    expect(reads, unorderedEquals([1, 2]));

    expect(find.byKey(const ValueKey('file-index.html')), findsOneWidget);
    expect(find.byKey(const ValueKey('file-app.css')), findsOneWidget);
    expect(find.byKey(const ValueKey('file-app.js')), findsOneWidget);
    expect(find.byKey(const ValueKey('file-notes.txt')), findsNothing);
    expect(find.text('Added'), findsOneWidget);
    expect(find.text('Removed'), findsOneWidget);
    expect(find.text('Changed'), findsOneWidget);
    // Line counts of the text diff on the changed file.
    expect(find.text('+1 −1'), findsOneWidget);
  });

  testWidgets('tapping a changed file shows coloured added and removed lines', (
    tester,
  ) async {
    final r = await _rig();
    await tester.pumpWidget(r.versionPage(2));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('file-index.html')));
    await tester.pumpAndSettle();

    final del = find.byKey(const ValueKey('del-a12'));
    final add = find.byKey(const ValueKey('add-b12'));
    expect(del, findsOneWidget);
    expect(add, findsOneWidget);
    expect(
      find.descendant(of: del, matching: find.text('<p>row 10</p>')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: add, matching: find.text('<p>row ten</p>')),
      findsOneWidget,
    );
    final scheme = claxTheme(Brightness.light).colorScheme;
    Color? fill(Finder f) =>
        (tester
                    .widget<Container>(
                      find
                          .descendant(of: f, matching: find.byType(Container))
                          .first,
                    )
                    .decoration
                as BoxDecoration?)
            ?.color;
    expect(fill(del), scheme.errorContainer);
    expect(fill(add), isNot(scheme.errorContainer));
    expect(fill(add), isNotNull);
    // The unchanged runs before and after the hunk are folded.
    expect(find.text('8 unchanged lines'), findsOneWidget);
    expect(find.text('7 unchanged lines'), findsOneWidget);
  });

  testWidgets('a file over 4,000 lines shows sizes only', (tester) async {
    final r = await _rig();
    await tester.pumpWidget(r.versionPage(3));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('file-data.csv')), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('file-data.csv')));
    await tester.pumpAndSettle();
    expect(find.textContaining('Too large to compare'), findsOneWidget);
    expect(find.byKey(const ValueKey('add-b1')), findsNothing);
  });

  testWidgets('Preview and Restore call back with the version', (tester) async {
    final r = await _rig();
    await tester.pumpWidget(r.versionPage(2));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('preview')));
    await tester.pump();
    expect(r.previews, [2]);
    await tester.tap(find.byKey(const ValueKey('restore')));
    await tester.pump();
    expect(r.restores, [2]);
  });

  testWidgets('the latest version offers neither Restore nor a comparison', (
    tester,
  ) async {
    final r = await _rig();
    await tester.pumpWidget(r.versionPage(3));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('restore')), findsNothing);
    expect(find.byKey(const ValueKey('compare-current')), findsNothing);
  });

  testWidgets('Compare with current diffs this version against the latest', (
    tester,
  ) async {
    final r = await _rig();
    await tester.pumpWidget(r.versionPage(1));
    await tester.pumpAndSettle();
    // Version 1 against nothing: every file is added.
    expect(find.text('Added'), findsNWidgets(3));

    await tester.tap(find.byKey(const ValueKey('compare-current')));
    await tester.pumpAndSettle();
    final reads = [
      for (final c in r.api.calls)
        if (c.name == 'readFiles') c.args['n'],
    ];
    expect(reads, contains(3));
    // v1 → v3: index.html changed, app.css removed, app.js and data.csv
    // added, notes.txt unchanged.
    expect(find.byKey(const ValueKey('file-notes.txt')), findsNothing);
    expect(find.text('Removed'), findsOneWidget);
    expect(find.text('Added'), findsNWidgets(2));
    expect(find.text('Changed'), findsOneWidget);
  });

  testWidgets('a failed read shows the error and retries', (tester) async {
    final r = await _rig();
    r.api.failNext(
      'readFiles',
      const ClaxApiError(
        status: 500,
        code: 'internal',
        message: 'disk on fire',
      ),
    );
    await tester.pumpWidget(r.versionPage(2));
    await tester.pumpAndSettle();
    expect(find.textContaining('disk on fire'), findsOneWidget);
    await tester.tap(find.text('Retry'));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('file-index.html')), findsOneWidget);
  });

  testWidgets('a Versions row opens its version page; Preview closes it', (
    tester,
  ) async {
    final r = await _rig();
    final detail = await r.api.getArtifact(r.aid);
    final previews = <int?>[];
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light),
        home: Scaffold(
          body: VersionsTab(
            api: r.api,
            detail: detail,
            error: null,
            shown: null,
            busy: false,
            onPreview: previews.add,
            onRestore: (_) {},
            onRetry: () {},
          ),
        ),
      ),
    );
    await tester.tap(find.byKey(const ValueKey('version-2')));
    await tester.pumpAndSettle();
    expect(find.byType(VersionPage), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('preview')));
    await tester.pumpAndSettle();
    expect(find.byType(VersionPage), findsNothing);
    expect(previews, [2]);

    // The latest previews as null, which follows new versions again.
    await tester.tap(find.byKey(const ValueKey('version-3')));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('preview')));
    await tester.pumpAndSettle();
    expect(previews, [2, null]);
  });
}
