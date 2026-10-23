import 'package:clax_app/clax/api.dart';
import 'package:clax_app/remote/models.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/versions_tab.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

ArtifactDetail _detail(int count) => ArtifactDetail(
  artifact: ArtifactSummary(
    id: 'a1',
    title: 'Plant log',
    currentVersion: count,
    updatedAt: '2026-10-21T10:00:00Z',
  ),
  versions: [
    for (var n = 1; n <= count; n++)
      VersionInfo(
        artifactId: 'a1',
        n: n,
        createdAt: '2026-10-21T10:00:00Z',
        files: const {},
        note: 'Change $n',
      ),
  ],
);

const _link = AppLink(
  remoteOrigin: 'https://clax.example',
  remoteArtifactId: 'ra1',
  url: 'https://clax.example/a/ra1',
  baseRemoteVersion: 2,
  role: AppLink.author,
  published: [1, 3],
);

void main() {
  test('marks: the last version published is public, earlier ones were, '
      'the rest are here; none for an app with no link', () {
    expect(versionMarks(_detail(4), _link), {
      1: VersionMark.wasPublic,
      2: VersionMark.here,
      3: VersionMark.public,
      4: VersionMark.here,
    });
    expect(versionMarks(_detail(4), null), isEmpty);
  });

  testWidgets('versions_tab_marks_public', (tester) async {
    final published = <int>[];
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
        home: Scaffold(
          body: VersionsTab(
            detail: _detail(4),
            error: null,
            shown: null,
            busy: false,
            marks: versionMarks(_detail(4), _link),
            onPublish: published.add,
            onPreview: (_) {},
            onRestore: (_) {},
            onRetry: () {},
          ),
        ),
      ),
    );
    String mark(int n) =>
        tester.widget<Text>(find.byKey(ValueKey('mark-$n'))).semanticsLabel!;
    expect(mark(3), 'public');
    expect(mark(1), 'was public');
    expect(mark(2), 'here');
    expect(mark(4), 'here');
    expect(
      find.byIcon(Icons.cloud_rounded),
      findsNWidgets(2),
      reason: 'the public and was-public marks draw the cloud as an icon',
    );

    // Publish on a row publishes that version; the public one offers none.
    expect(find.byKey(const ValueKey('publish-3')), findsNothing);
    expect(
      tester
          .widget<Text>(
            find.descendant(
              of: find.byKey(const ValueKey('publish-2')),
              matching: find.byType(Text),
            ),
          )
          .semanticsLabel,
      'Publish version 2',
    );
    await tester.tap(find.byKey(const ValueKey('publish-2')));
    expect(published, [2]);
  });

  testWidgets('without marks the rows show none and offer no Publish', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          body: VersionsTab(
            detail: _detail(2),
            error: null,
            shown: null,
            busy: false,
            onPreview: (_) {},
            onRestore: (_) {},
            onRetry: () {},
          ),
        ),
      ),
    );
    expect(find.byKey(const ValueKey('mark-1')), findsNothing);
    expect(find.byKey(const ValueKey('publish-1')), findsNothing);
  });
}
