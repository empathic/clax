import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/threads_panel.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  group('the composer peek fits its height at every text scale', () {
    Future<void> pumpPeek(
      WidgetTester tester, {
      required double scale,
      double width = 320,
    }) async {
      tester.view.devicePixelRatio = 3;
      tester.view.physicalSize = Size(width, 640) * 3;
      addTearDown(tester.view.reset);
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light),
          home: MediaQuery.withClampedTextScaling(
            minScaleFactor: scale,
            maxScaleFactor: scale,
            child: Scaffold(
              body: Align(
                alignment: Alignment.bottomCenter,
                child: SizedBox(
                  height: AppPanel.peekHeight,
                  child: AppPanel(
                    wide: false,
                    tab: PanelTab.threads,
                    onTab: (_) {},
                    onClose: () {},
                    draft: const Draft(
                      label: 'The Mon run card',
                      text: '',
                      clip: DraftClip.ready,
                    ),
                    busy: false,
                    dictation: const Dictation(
                      listening: true,
                      words:
                          'make the Monday card show the pace per kilometre '
                          'and the heart rate under it',
                    ),
                    onMic: () {},
                    threads: const [],
                    onThread: (_) {},
                    onResend: (_) {},
                    versions: const SizedBox(),
                  ),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pump();
    }

    int statusLines(WidgetTester tester) => tester
        .widget<Text>(find.byKey(const ValueKey('peek-status')))
        .maxLines!;

    testWidgets('1.0: label and two status lines', (tester) async {
      await pumpPeek(tester, scale: 1.0);
      expect(tester.takeException(), isNull);
      expect(find.byKey(const ValueKey('peek-label')), findsOneWidget);
      expect(statusLines(tester), 2);
    });

    testWidgets('1.3: label and two status lines', (tester) async {
      await pumpPeek(tester, scale: 1.3);
      expect(tester.takeException(), isNull);
      expect(find.byKey(const ValueKey('peek-label')), findsOneWidget);
      expect(statusLines(tester), 2);
    });

    testWidgets('2.0 on 320 dp: no overflow, the status kept', (tester) async {
      await pumpPeek(tester, scale: 2.0);
      expect(tester.takeException(), isNull);
      expect(find.byKey(const ValueKey('peek-status')), findsOneWidget);
      expect(statusLines(tester), 1);
    });
  });

  testWidgets(
    'a wide panel composing beside an unsent thread offers Send to Clax, '
    'not a second Send',
    (tester) async {
      tester.view.devicePixelRatio = 2;
      tester.view.physicalSize = const Size(1200, 800) * 2;
      addTearDown(tester.view.reset);
      final resent = <String>[];
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
                  draft: const Draft(
                    label: '“Mon 5 km”',
                    text: '',
                    clip: DraftClip.ready,
                  ),
                  busy: false,
                  dictation: const Dictation(listening: false, words: ''),
                  onMic: () {},
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
                  onThread: (_) {},
                  onResend: resent.add,
                  versions: const SizedBox(),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pump();
      expect(find.byKey(const ValueKey('peek-label')), findsOneWidget);
      expect(find.text('Send'), findsNothing);
      final row = find.byKey(const ValueKey('resend-t1'));
      expect(
        find.descendant(of: row, matching: find.text('Send to Clax')),
        findsOneWidget,
      );
      await tester.tap(row);
      expect(resent, ['t1']);
    },
  );
}
