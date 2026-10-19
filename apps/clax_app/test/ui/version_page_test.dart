import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/version_page.dart';
import 'package:clax_app/ui/versions_tab.dart';
import 'package:flutter/foundation.dart';
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
    theme: claxTheme(Brightness.light, defaultTargetPlatform),
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
  // Versions without hashes: the page tells changes apart by their texts.
  final api = FakeClaxApi(clock: () => '2026-10-12T09:30:00Z')
    ..contentHashes = false;
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
        'index.html': page(v2Body),
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
    final colors = DiffColors.of(
      claxTheme(Brightness.light, defaultTargetPlatform),
    );
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
    expect(fill(del), colors.del);
    expect(fill(add), colors.add);
    expect(colors.add, isNot(colors.del));
    expect(find.text('+1  −1', findRichText: true), findsOneWidget);
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
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
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

  testWidgets('reads no file until one is tapped when the metadata settles', (
    tester,
  ) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T09:30:00Z');
    final aid = api.seedArtifact('Runs', {'index.html': page(v1Body)});
    await api.registerAppSession(installId: 'i', pid: 1);
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        files: {
          'index.html': page([...v1Body, '<p>more</p>']),
        },
      ),
    );
    final r = _Rig(api, aid);
    await tester.pumpWidget(r.versionPage(2));
    await tester.pumpAndSettle();
    List<Object?> reads() => [
      for (final c in api.calls)
        if (c.name == 'readFiles') c.args['n'],
    ];
    expect(find.byKey(const ValueKey('file-index.html')), findsOneWidget);
    expect(reads(), isEmpty);

    await tester.tap(find.byKey(const ValueKey('file-index.html')));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('add-b23')), findsOneWidget);
    expect(reads(), unorderedEquals([1, 2]));

    await tester.pageBack();
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('file-index.html')));
    await tester.pumpAndSettle();
    expect(reads(), hasLength(2), reason: 'each version is read once');
  });

  testWidgets('same-size files without hashes are compared by their text', (
    tester,
  ) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T09:30:00Z');
    final aid = api.seedArtifact('Runs', {
      'index.html': page(v1Body),
      'a.txt': 'keep\n',
      'b.txt': 'same\n',
    });
    await api.registerAppSession(installId: 'i', pid: 1);
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        files: {'index.html': page(v1Body), 'a.txt': 'kept\n'},
      ),
    );
    await tester.pumpWidget(_Rig(api, aid).versionPage(2));
    await tester.pumpAndSettle();
    expect(find.byKey(const ValueKey('file-a.txt')), findsOneWidget);
    expect(find.byKey(const ValueKey('file-b.txt')), findsNothing);
    expect(find.byKey(const ValueKey('file-index.html')), findsNothing);
  });

  testWidgets('a change of line endings or final newline only says so', (
    tester,
  ) async {
    final api = FakeClaxApi(clock: () => '2026-10-12T09:30:00Z');
    final aid = api.seedArtifact('Runs', {
      'index.html': page(v1Body),
      'notes.txt': 'a\nb',
    });
    await api.registerAppSession(installId: 'i', pid: 1);
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        files: {'index.html': page(v1Body), 'notes.txt': 'a\r\nb\n'},
      ),
    );
    await tester.pumpWidget(_Rig(api, aid).versionPage(2));
    await tester.pumpAndSettle();
    await tester.tap(find.byKey(const ValueKey('file-notes.txt')));
    await tester.pumpAndSettle();
    expect(
      find.text('Only line endings or the final newline changed.'),
      findsOneWidget,
    );
  });

  testWidgets('a long path shows its file name over its directory', (
    tester,
  ) async {
    const path =
        'src/components/very/deeply/nested/folder/with-a-long-file-name.component.js';
    final api = FakeClaxApi(clock: () => '2026-10-12T09:30:00Z');
    final aid = api.seedArtifact('Runs', {'index.html': page(v1Body)});
    await api.registerAppSession(installId: 'i', pid: 1);
    await api.publish(
      aid,
      PublishRequest(
        ifVersion: 1,
        files: {'index.html': page(v1Body), path: 'x()\n'},
      ),
    );
    await tester.pumpWidget(_Rig(api, aid).versionPage(2));
    await tester.pumpAndSettle();
    final row = find.byKey(const ValueKey('file-$path'));
    expect(
      find.descendant(
        of: row,
        matching: find.text('with-a-long-file-name.component.js'),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(
        of: row,
        matching: find.text('src/components/very/deeply/nested/folder/'),
      ),
      findsOneWidget,
    );
  });

  test(
    'a large pair is diffed off the UI isolate with the same result',
    () async {
      final a = [for (var i = 0; i < 900; i++) 'line $i'];
      final b = [...a]..[450] = 'changed';
      final d = await diffTexts(a.join('\n'), b.join('\n'));
      expect(d.hunks, hasLength(1));
      expect(a.length + b.length, greaterThan(diffIsolateLines));
      expect(d.added, 1);
      expect(d.removed, 1);
      expect(d.endingsOnly, isFalse);
    },
  );

  test('a present side without text is not a text file', () async {
    final d = await diffTexts(null, 'x');
    expect(d.hunks, isNull);
    expect(d.why, 'Not a text file');
    final added = await diffTexts(null, 'x', fromPresent: false);
    expect(added.added, 1);
  });
}
