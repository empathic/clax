import 'dart:convert';
import 'dart:ui' as ui;

import 'package:clax_app/ui/comment_composer.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/threads_panel.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// A 1 x 1 PNG.
final _png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
);

/// A [width] x [height] PNG of one colour.
Future<Uint8List> _pngOf(int width, int height) async {
  final rec = ui.PictureRecorder();
  ui.Canvas(rec).drawRect(
    Rect.fromLTWH(0, 0, width.toDouble(), height.toDouble()),
    ui.Paint()..color = const Color(0xFF3366CC),
  );
  final image = await rec.endRecording().toImage(width, height);
  final data = await image.toByteData(format: ui.ImageByteFormat.png);
  image.dispose();
  return data!.buffer.asUint8List();
}

ThreadSummary _t(
  String id, {
  String status = 'open',
  String summary = 'make it blue',
  bool sent = false,
  int comments = 1,
  String label = '',
  bool clip = false,
}) => ThreadSummary(
  id: id,
  status: status,
  summary: summary,
  sent: sent,
  comments: comments,
  version: 1,
  label: label,
  clip: clip,
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
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
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
      expect(tester.widget(row), isA<IconButton>());
      expect((tester.widget(row) as IconButton).tooltip, 'Send to Clax');
      expect(find.text('Send to Clax'), findsNothing, reason: 'an icon');
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

  testWidgets('a thread is titled by what it is on, with a short meta line', (
    tester,
  ) async {
    await pumpPanel(
      tester,
      threads: [
        _t('t1', label: 'Chart under “p95 by four-hour window”', comments: 3),
        _t('t2', summary: 'no anchor words', sent: true),
      ],
    );
    final t1 = find.byKey(const ValueKey('thread-t1'));
    expect(
      find.descendant(
        of: t1,
        matching: find.text('Chart under “p95 by four-hour window”'),
      ),
      findsOneWidget,
    );
    expect(
      find.descendant(of: t1, matching: find.text('make it blue')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: t1, matching: find.text('3 comments · not sent')),
      findsOneWidget,
    );
    final t2 = find.byKey(const ValueKey('thread-t2'));
    expect(
      find.descendant(of: t2, matching: find.text('no anchor words')),
      findsOneWidget,
    );
    expect(
      find.descendant(of: t2, matching: find.text('1 comment · sent to Clax')),
      findsOneWidget,
    );
    expect(find.textContaining('v1'), findsNothing);
  });

  testWidgets('open threads come first, then a collapsed Resolved (n) in muted', (
    tester,
  ) async {
    await pumpPanel(
      tester,
      threads: [
        _t('r1', status: 'resolved', summary: 'done one'),
        _t('o1', summary: 'older open'),
        _t('r2', status: 'resolved', summary: 'done two', comments: 2),
        _t('o2', summary: 'newer open'),
      ],
    );
    final header = find.text('Resolved (2)');
    expect(header, findsOneWidget);
    final top = (Finder f) => tester.getTopLeft(f).dy;
    expect(top(find.text('newer open')), lessThan(top(find.text('older open'))));
    expect(top(find.text('older open')), lessThan(top(header)));
    expect(find.text('done one'), findsNothing, reason: 'collapsed');
    expect(find.byKey(const ValueKey('thread-r2')), findsNothing);
    final muted = ClaxTokens.light.muted;
    expect(tester.widget<Text>(header).style?.color, muted);
    await tester.tap(header);
    await tester.pumpAndSettle();
    expect(top(header), lessThan(top(find.text('done two'))));
    expect(top(find.text('done two')), lessThan(top(find.text('done one'))));
    expect(tester.widget<Text>(find.text('done two')).style?.color, muted);
    expect(find.text('2 comments'), findsOneWidget);
  });

  testWidgets('a wide clip gets its own line at the panel width, cropped to 16:9; '
      'a narrow one stays beside the title', (tester) async {
    final wide = await tester.runAsync(() => _pngOf(1600, 400));
    final narrow = await tester.runAsync(() => _pngOf(400, 300));
    await pumpPanel(
      tester,
      threads: [
        _t('w', label: '“Mon 5 km”', clip: true),
        _t('n', label: 'Image', clip: true),
      ],
      clipOf: (id) async => id == 'w' ? wide : narrow,
    );
    await tester.pump();
    final w = find.byKey(const ValueKey('thread-clip-w'));
    expect(w, findsOneWidget);
    final box = tester.getSize(w);
    expect(box.width, greaterThan(300), reason: 'the panel is 360 wide');
    expect(box.width / box.height, moreOrLessEquals(16 / 9, epsilon: 0.01));
    final title = find.text('“Mon 5 km”');
    expect(tester.getTopLeft(w).dy, greaterThan(tester.getBottomLeft(title).dy));
    final n = find.byKey(const ValueKey('thread-clip-n'));
    expect(tester.getSize(n).width, lessThanOrEqualTo(56));
    expect(
      tester.getTopLeft(n).dx,
      lessThan(tester.getTopLeft(find.text('Image')).dx),
    );
  });

  testWidgets('the empty panel says how to comment in the pointer\'s words', (
    tester,
  ) async {
    await pumpPanel(tester);
    final verb = defaultTargetPlatform == TargetPlatform.macOS ? 'Click' : 'Tap';
    expect(find.text('$verb anything in the app to comment on it.'), findsOneWidget);
  });
}
