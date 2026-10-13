import 'dart:io';

import 'package:clax_app/ui/comment_bar.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/threads_panel.dart';
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
    expect(find.text('Hold on what you want changed'), findsOneWidget);
    expect(find.text('Threads (2)'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('comment-whole-app')));
    await tester.tap(find.byKey(const ValueKey('comment-threads')));
    await tester.tap(find.byKey(const ValueKey('comment-voice')));
    await tester.tap(find.byKey(const ValueKey('comment-bar-close')));
    expect(log, ['whole', 'threads', 'voice', 'close']);
  });

  group('the comment bar fits', () {
    setUpAll(() async {
      if (_fonts != null) await _loadRoboto();
    });

    for (final (width, scale) in [
      (320.0, 1.0),
      (320.0, 1.3),
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
            theme: claxTheme(Brightness.light).copyWith(
              textTheme: claxTheme(Brightness.light).textTheme
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
        for (final t in [
          'Whole app',
          'Threads (12)',
          'Hold on what you want changed',
        ]) {
          final p = tester.renderObject<RenderParagraph>(find.text(t));
          expect(p.didExceedMaxLines, isFalse, reason: '"$t" is cut');
        }
        final bar = tester.getRect(find.byKey(const ValueKey('comment-bar')));
        // The composer's peek takes the bar's place at the same height.
        expect(bar.height, AppPanel.peekHeight + 34);
        // The actions end the bar, at any width.
        final close = tester.getRect(
          find.byKey(const ValueKey('comment-bar-close')),
        );
        expect(close.right, greaterThan(width - 16));
        final threads = tester.getRect(
          find.byKey(const ValueKey('comment-threads')),
        );
        expect(threads.right, lessThanOrEqualTo(close.left));
        // Voice ends the actions, or the hint's line when they are tight.
        final voice = tester.getRect(
          find.byKey(const ValueKey('comment-voice')),
        );
        expect(bar.top, lessThanOrEqualTo(voice.top));
        expect(voice.bottom, lessThanOrEqualTo(bar.bottom - 34));
        expect(voice.overlaps(threads), isFalse);
        expect(voice.overlaps(close), isFalse);
        expect(voice.right, lessThanOrEqualTo(width));
      }, skip: _fonts == null);
    }
  });
}
