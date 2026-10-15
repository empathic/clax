import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/ui/comment_composer.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/threads_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// A 1 x 1 PNG.
final _png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
);

void main() {
  Future<void> pumpPanel(
    WidgetTester tester, {
    Widget? composer,
    List<ThreadSummary> threads = const [],
    ValueChanged<String>? onResend,
    Future<Uint8List?> Function(String id)? clipOf,
  }) async {
    tester.view.devicePixelRatio = 2;
    tester.view.physicalSize = const Size(1200, 800) * 2;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light),
        home: Scaffold(
          body: Align(
            alignment: Alignment.centerRight,
            child: SizedBox(
              width: 360,
              child: AppPanel(
                wide: true,
                tab: PanelTab.threads,
                onTab: (_) {},
                onClose: () {},
                composer: composer,
                threads: threads,
                onThread: (_) {},
                onResend: onResend ?? (_) {},
                versions: const SizedBox(),
                clipOf: clipOf,
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  testWidgets(
    'a wide panel composing beside an unsent thread has the composer on top, '
    'one Send, and Send to Clax on the thread',
    (tester) async {
      final resent = <String>[];
      final text = TextEditingController();
      addTearDown(text.dispose);
      await pumpPanel(
        tester,
        composer: CommentComposer(
          draft: const Draft(
            key: 'p1',
            label: '“Mon 5 km”',
            quote: 'Mon 5 km',
            text: '',
            clip: DraftClip.ready,
          ),
          controller: text,
          dictation: const Dictation(),
          onMic: () {},
          onSend: () {},
          onCancel: () {},
          docked: false,
        ),
        threads: const [
          ThreadSummary(
            id: 't1',
            status: 'open',
            summary: 'make it blue',
            sent: false,
            comments: 1,
            version: 1,
          ),
        ],
        onResend: resent.add,
      );
      expect(find.byKey(const ValueKey('composer')), findsOneWidget);
      expect(find.text('Threads'), findsNothing, reason: 'in the header');
      expect(find.text('Send'), findsOneWidget);
      final row = find.byKey(const ValueKey('resend-t1'));
      expect(
        find.descendant(of: row, matching: find.text('Send to Clax')),
        findsOneWidget,
      );
      await tester.tap(row);
      expect(resent, ['t1']);
    },
  );

  testWidgets("a thread with a screenshot shows it in the list, read once; "
      'one without shows its icon', (tester) async {
    final asked = <String>[];
    await pumpPanel(
      tester,
      threads: const [
        ThreadSummary(
          id: 't1',
          status: 'open',
          summary: 'bigger',
          sent: true,
          comments: 1,
          version: 1,
          label: '“Mon 5 km”',
          clip: true,
        ),
        ThreadSummary(
          id: 't2',
          status: 'open',
          summary: 'all of it',
          sent: true,
          comments: 1,
          version: 1,
          label: 'Whole app',
        ),
      ],
      clipOf: (id) async {
        asked.add(id);
        return _png;
      },
    );
    await tester.pump();
    expect(asked, ['t1']);
    expect(find.byKey(const ValueKey('thread-clip-t1')), findsOneWidget);
    expect(find.byKey(const ValueKey('thread-clip-t2')), findsNothing);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('thread-t2')),
        matching: find.byIcon(Icons.chat_bubble_outline),
      ),
      findsOneWidget,
    );
  });
}
