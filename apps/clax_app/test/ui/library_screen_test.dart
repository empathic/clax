import 'dart:typed_data';

import 'package:clax_app/clax/api.dart' show ClaxApiError;
import 'package:clax_app/remote/fake_remote_api.dart';
import 'package:clax_app/remote/puller.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/remote/library_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

String _h(int i) => 'sha256:${i.toRadixString(16).padLeft(64, '0')}';

void main() {
  testWidgets('library_lists_and_pulls_selected', (tester) async {
    final remote = FakeRemoteApi();
    final r = remote.connected('https://clax.dev');
    final mine = remote.addCopy(
      r.id,
      contentSha256: _h(1),
      title: 'Plant log',
      access: 'owner',
      general: 'restricted',
    );
    final shared = remote.addCopy(
      r.id,
      contentSha256: _h(2),
      title: 'Run tracker',
      access: 'commenter',
    );
    remote.addCopy(r.id, contentSha256: _h(3), title: 'No bundle yet');
    for (final (c, bv) in [(mine, 4), (shared, 2)]) {
      c.versions.single.bundle = FakeBundle(
        bytes: Uint8List(0),
        lineage: 'l_${c.raid}',
        bundleVersion: bv,
        contentSha256: c.versions.single.contentSha256,
      );
    }
    final pulled = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        home: LibraryScreen(
          remote: remote,
          rid: r.id,
          pull: (app) async {
            pulled.add(app.artifactId);
            return const PullInstalled('a_new');
          },
        ),
      ),
    );
    await tester.pumpAndSettle();

    expect(find.text('Plant log'), findsOneWidget);
    expect(find.text('Run tracker'), findsOneWidget);
    expect(find.text('No bundle yet'), findsNothing);
    expect(find.text('Yours · v4'), findsOneWidget);
    expect(find.text('Shared with you · can comment · v2'), findsOneWidget);
    expect(find.text('Apps come without their data.'), findsOneWidget);

    // Nothing is picked at first, so nothing can be got.
    final get = find.byKey(const ValueKey('library-get'));
    expect(tester.widget<ClaxButton>(get).onPressed, isNull);

    await tester.tap(find.byKey(ValueKey('pick-${shared.raid}')));
    await tester.pump();
    await tester.tap(get);
    await tester.pumpAndSettle();
    expect(pulled, [shared.raid]);
    expect(find.text('Got it'), findsOneWidget);
  });

  testWidgets('a library the remote cannot give says why: out of reach, or '
      'too old for the app', (tester) async {
    final remote = FakeRemoteApi();
    final r = remote.connected('https://clax.dev');
    remote.failNext(
      'apps',
      const ClaxApiError(
        status: 502,
        code: 'remote_unreachable',
        message: 'https://clax.dev could not be reached',
      ),
    );
    Widget screen() => MaterialApp(
      home: LibraryScreen(
        key: UniqueKey(),
        remote: remote,
        rid: r.id,
        pull: (app) async => const PullCancelled(),
      ),
    );
    await tester.pumpWidget(screen());
    await tester.pumpAndSettle();
    expect(
      find.text('Couldn’t reach clax.dev. Check your connection and try again.'),
      findsOneWidget,
    );

    remote.tooOld.add('https://clax.dev');
    await tester.pumpWidget(screen());
    await tester.pumpAndSettle();
    expect(
      find.text('clax.dev needs an update before the Clax app can list your '
          'apps.'),
      findsOneWidget,
    );
  });
}
