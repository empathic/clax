import 'dart:io';

import 'package:clax_app/ui/comment_bar.dart';
import 'package:clax_app/ui/kit/button.dart';
import 'package:clax_app/ui/kit/verbs.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/threads_panel.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// Flutter's own Roboto, so text is measured as on a phone (the test font
/// draws every glyph a full em wide); null outside `flutter test`.
String? get _fonts {
  final root = Platform.environment['FLUTTER_ROOT'];
  if (root == null) return null;
  final dir = '$root/bin/cache/artifacts/material_fonts';
  return File('$dir/Roboto-Regular.ttf').existsSync() ? dir : null;
}

Future<void> _loadRoboto() async {
  final dir = _fonts!;
  final l = FontLoader('Roboto');
  for (final f in ['Roboto-Regular.ttf', 'Roboto-Medium.ttf']) {
    final bytes = File('$dir/$f').readAsBytesSync();
    l.addFont(Future.value(ByteData.view(bytes.buffer)));
  }
  await l.load();
}

void main() {
  testWidgets('the comment bar calls back and counts threads', (tester) async {
    final log = <String>[];
    await tester.pumpWidget(
      MaterialApp(
        home: Scaffold(
          bottomNavigationBar: CommentBar(
            threadCount: 2,
            onWholeApp: () => log.add('whole'),
            onThreads: () => log.add('threads'),
            dictate: true,
            onVoice: () => log.add('voice'),
            onClose: () => log.add('close'),
          ),
        ),
      ),
    );
    // A tap on the page is the default; the general comment is one
    // labelled button.
    expect(find.text('Tap anything to comment on it'), findsOneWidget);
    expect(find.text('Whole app'), findsOneWidget);
    expect(find.text('Threads (2)'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('comment-whole-app')));
    await tester.tap(find.byKey(const ValueKey('comment-threads')));
    await tester.tap(find.byKey(const ValueKey('comment-voice')));
    await tester.tap(find.byKey(const ValueKey('comment-bar-close')));
    expect(log, ['whole', 'threads', 'voice', 'close']);
  });

  testWidgets('the hint says Click once a mouse is in use, and the general '
      'comment is a tonal button', (tester) async {
    await tester.pumpWidget(
      ClaxPointerScope(
        child: MaterialApp(
          theme: claxTheme(Brightness.light, TargetPlatform.android),
          home: Scaffold(
            bottomNavigationBar: CommentBar(
              threadCount: 0,
              onWholeApp: () {},
              onThreads: () {},
              dictate: false,
              onVoice: () {},
              onClose: () {},
            ),
          ),
        ),
      ),
    );
    expect(find.text('Tap anything to comment on it'), findsOneWidget);
    final whole = tester.widget<ClaxButton>(
      find.byKey(const ValueKey('comment-whole-app')),
    );
    expect(whole.kind, ClaxButtonKind.tonal);
    final mouse = await tester.createGesture(kind: PointerDeviceKind.mouse);
    await mouse.addPointer(location: const Offset(10, 10));
    await mouse.moveTo(const Offset(20, 20));
    await tester.pump();
    expect(find.text('Click anything to comment on it'), findsOneWidget);
    await mouse.removePointer();
  });

  Future<void> pumpBar(
    WidgetTester tester, {
    double width = 390,
    bool waiting = false,
    Brightness brightness = Brightness.light,
  }) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = Size(width, 700);
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(brightness, TargetPlatform.android),
        home: Scaffold(
          bottomNavigationBar: CommentBar(
            threadCount: 12,
            waiting: waiting,
            onWholeApp: () {},
            onThreads: () {},
            dictate: false,
            onVoice: () {},
            onClose: () {},
          ),
        ),
      ),
    );
  }

  for (final brightness in Brightness.values) {
    testWidgets('comment mode is people’s red-orange: the top rule and the '
        'touch icon are tertiary (${brightness.name})', (tester) async {
      await pumpBar(tester, width: 800, brightness: brightness);
      final scheme = claxTheme(brightness, TargetPlatform.android).colorScheme;
      final bar = tester.widget<Material>(
        find.byKey(const ValueKey('comment-bar')),
      );
      expect((bar.shape! as Border).top.color, scheme.tertiary);
      final touch = tester.widget<Icon>(find.byIcon(Icons.touch_app_outlined));
      expect(touch.color, scheme.tertiary);
    });
  }

  for (final width in [340.0, 390.0, 412.0]) {
    testWidgets('one row of 48 dp targets with 24 dp glyphs at $width dp', (
      tester,
    ) async {
      await pumpBar(tester, width: width);
      final keys = [
        'comment-whole-app',
        'comment-threads',
        'comment-voice',
        'comment-bar-close',
      ];
      final rects = [
        for (final k in keys) tester.getRect(find.byKey(ValueKey(k))),
      ];
      final hint = tester.getRect(find.byKey(const ValueKey('comment-hint')));
      for (final (i, r) in rects.indexed) {
        expect(r.width, greaterThanOrEqualTo(48), reason: keys[i]);
        expect(r.height, greaterThanOrEqualTo(48), reason: keys[i]);
        // In order, after the instruction, on one line.
        expect(r.left, greaterThanOrEqualTo(hint.right), reason: keys[i]);
        if (i > 0) expect(r.left, greaterThanOrEqualTo(rects[i - 1].right));
        expect(r.center.dy, closeTo(rects[0].center.dy, 0.5));
      }
      for (final k in keys.skip(2)) {
        final icon = tester.widget<Icon>(
          find.descendant(
            of: find.byKey(ValueKey(k)),
            matching: find.byType(Icon),
          ),
        );
        expect(icon.size ?? 24, 24, reason: k);
      }
      // Below 360 dp Threads is its icon and count only.
      if (width < 360) {
        expect(find.text('Threads (12)'), findsNothing);
        expect(find.byTooltip('Threads (12)'), findsOneWidget);
        expect(find.text('12'), findsOneWidget);
      }
      expect(tester.takeException(), isNull);
    });
  }

  testWidgets('a slow page: the bar says Waiting for the page… in the '
      'instruction’s place', (tester) async {
    await pumpBar(tester, waiting: true);
    expect(find.text('Waiting for the page…'), findsOneWidget);
    expect(find.text('Tap anything to comment on it'), findsNothing);
    await pumpBar(tester);
    expect(find.text('Waiting for the page…'), findsNothing);
    expect(find.text('Tap anything to comment on it'), findsOneWidget);
  });

  group('the comment bar fits', () {
    setUpAll(() async {
      if (_fonts != null) await _loadRoboto();
    });

    for (final (width, scale) in [
      (320.0, 1.0),
      (320.0, 1.3),
      (340.0, 1.0),
      (390.0, 1.0),
      (390.0, 1.3),
      (1100.0, 1.0),
      (1100.0, 1.3),
    ]) {
      testWidgets('at $width dp and text scale $scale', (tester) async {
        tester.view.devicePixelRatio = 1;
        tester.view.physicalSize = Size(width, 700);
        tester.view.padding = const FakeViewPadding(bottom: 34);
        addTearDown(tester.view.reset);
        await tester.pumpWidget(
          MaterialApp(
            theme: claxTheme(Brightness.light, defaultTargetPlatform).copyWith(
              textTheme: claxTheme(Brightness.light, defaultTargetPlatform).textTheme
                  .apply(fontFamily: 'Roboto'),
            ),
            builder: (context, child) => MediaQuery(
              data: MediaQuery.of(context)
                  .copyWith(textScaler: TextScaler.linear(scale)),
              child: child!,
            ),
            home: Scaffold(
              bottomNavigationBar: CommentBar(
                threadCount: 12,
                onWholeApp: () {},
                onThreads: () {},
                dictate: false,
                onVoice: () {},
                onClose: () {},
              ),
            ),
          ),
        );
        // The instruction keeps its words, in full where the row has
        // room for them at the text size, else as "Tap to comment".
        final hint = tester.renderObject<RenderParagraph>(
          find.byKey(const ValueKey('comment-hint')),
        );
        expect(hint.didExceedMaxLines, isFalse, reason: 'the hint is cut');
        if (width >= 340 || scale == 1.0) {
          expect(find.text('Tap anything to comment on it'), findsOneWidget);
        } else {
          expect(find.text('Tap to comment'), findsOneWidget);
        }
        // The general comment keeps its label where it fits, else it is
        // an icon named by its tooltip.
        if (find.text(CommentBar.wholeApp).evaluate().isNotEmpty) {
          final p = tester.renderObject<RenderParagraph>(
            find.text(CommentBar.wholeApp),
          );
          expect(p.didExceedMaxLines, isFalse, reason: 'Whole app is cut');
        } else {
          expect(find.byTooltip(CommentBar.wholeAppTip), findsOneWidget);
          expect(width < 360 || scale > 1, isTrue);
        }
        // Threads keeps its label where it fits, else it is an icon with
        // its count, named by its tooltip; always so below 360 dp.
        if (find.text('Threads (12)').evaluate().isNotEmpty) {
          expect(width, greaterThanOrEqualTo(360));
          final p = tester.renderObject<RenderParagraph>(
            find.text('Threads (12)'),
          );
          expect(p.didExceedMaxLines, isFalse, reason: 'Threads is cut');
        } else {
          expect(find.byTooltip('Threads (12)'), findsOneWidget);
          expect(find.text('12'), findsOneWidget);
        }
        final bar = tester.getRect(find.byKey(const ValueKey('comment-bar')));
        // On a phone the composer's peek takes the bar's place at the
        // same height; wide, the bar is one line.
        expect(
          bar.height,
          (width < CommentBar.wideWidth
                  ? AppPanel.peekHeight
                  : CommentBar.wideHeight) +
              34,
        );
        final rects = [
          for (final k in [
            'comment-hint',
            'comment-whole-app',
            'comment-threads',
            'comment-voice',
            'comment-bar-close',
          ])
            tester.getRect(find.byKey(ValueKey(k))),
        ];
        // Everything is in the bar, above the inset, and nothing overlaps.
        for (final (i, r) in rects.indexed) {
          expect(r.left, greaterThanOrEqualTo(0));
          expect(r.right, lessThanOrEqualTo(width));
          expect(r.top, greaterThanOrEqualTo(bar.top));
          expect(r.bottom, lessThanOrEqualTo(bar.bottom - 34));
          for (final o in rects.skip(i + 1)) {
            expect(r.overlaps(o), isFalse, reason: '$r overlaps $o');
          }
        }
        // One row, and every control a 48 dp target.
        for (final r in rects.skip(1)) {
          expect(r.width, greaterThanOrEqualTo(48));
          expect(r.height, greaterThanOrEqualTo(48));
          expect(r.center.dy, closeTo(rects[2].center.dy, 0.5));
        }
        expect(tester.takeException(), isNull);
      }, skip: _fonts == null);
    }
  });
}
