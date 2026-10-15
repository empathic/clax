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
}
