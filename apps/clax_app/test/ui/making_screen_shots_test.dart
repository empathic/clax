// Renders the making screen at phone size, light and dark, at the stages a
// new app goes through, for a person to look at. Off by default; run with
//   CLAX_U1_SHOTS=1 flutter test --update-goldens test/ui/making_screen_shots_test.dart
// after rendering the page images into target/flutter-app/u1/src (the
// streamed page as Chromium shows it with JavaScript off).
import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/ui/making_preview.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

final _on = Platform.environment['CLAX_U1_SHOTS'] == '1';
// Goldens resolve from this file; file reads from the package directory.
const _out = '../../../../target/flutter-app/u1';
const _src = '../../target/flutter-app/u1/src';

Future<void> _fonts() async {
  final root = Platform.environment['FLUTTER_ROOT']!;
  final dir = '$root/bin/cache/artifacts/material_fonts';
  Future<ByteData> f(String n) async =>
      ByteData.sublistView(await File('$dir/$n').readAsBytes());
  final roboto = FontLoader('Roboto')
    ..addFont(f('Roboto-Regular.ttf'))
    ..addFont(f('Roboto-Medium.ttf'))
    ..addFont(f('Roboto-Bold.ttf'));
  await roboto.load();
  await (FontLoader(
    'MaterialIcons',
  )..addFont(f('MaterialIcons-Regular.otf'))).load();
}

class _ShotPreview implements PreviewView {
  _ShotPreview(this.frames);

  /// The page image for each loaded text, by the text it was loaded with.
  final Map<String, Uint8List> frames;
  final _shown = ValueNotifier<Uint8List?>(null);

  @override
  Future<void> loadHtml(String html) async => _shown.value = frames[html];

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: _shown,
    builder: (_, png, _) => png == null
        ? const SizedBox()
        : Image.memory(
            png,
            fit: BoxFit.fitWidth,
            alignment: Alignment.topCenter,
            gaplessPlayback: true,
          ),
  );

  @override
  void dispose() {}
}

/// The test's fake time, which [WidgetTester.pump] moves.
class _TestClock implements Clock {
  _TestClock(this.tester);

  final WidgetTester tester;

  @override
  DateTime now() => tester.binding.clock.now();

  @override
  Future<void> delay(Duration d) => Future<void>.delayed(d);
}

void main() {
  for (final dark in [false, true]) {
    final scheme = dark ? 'dark' : 'light';
    testWidgets('making screen stages, $scheme', skip: !_on, (tester) async {
      late Map<String, Uint8List> pages;
      await tester.runAsync(() async {
        await _fonts();
        pages = {
          for (final s in ['early', 'mid', 'full'])
            s: await File('$_src/page-$s-$scheme.png').readAsBytes(),
        };
      });
      tester.view.physicalSize = const Size(1080, 2340);
      tester.view.devicePixelRatio = 1080 / 412;
      addTearDown(tester.view.reset);
      final preview = _ShotPreview({
        'early': pages['early']!,
        'mid': pages['mid']!,
      });
      void Function(MakeUpdate)? report;
      final done = Completer<String>();
      await tester.pumpWidget(
        MaterialApp(
          debugShowCheckedModeBanner: false,
          theme: claxTheme(Brightness.light),
          darkTheme: claxTheme(Brightness.dark),
          themeMode: dark ? ThemeMode.dark : ThemeMode.light,
          home: MakingScreen(
            request: 'a focus timer with a log of today’s sessions',
            make: (_, _, r) {
              report = r;
              return done.future;
            },
            preview: () => preview,
            clock: _TestClock(tester),
          ),
        ),
      );
      Future<void> shot(String name) async {
        await tester.pump(const Duration(milliseconds: 300));
        await tester.runAsync(() async {
          for (final e in find.byType(Image).evaluate()) {
            await precacheImage((e.widget as Image).image, e);
          }
        });
        await tester.pump(const Duration(milliseconds: 400));
        await expectLater(
          find.byType(MakingScreen),
          matchesGoldenFile('$_out/making-$name-$scheme.png'),
        );
      }

      void p(AgentProgress e) => report!(MakeProgress(e));

      await tester.pump(const Duration(seconds: 1));
      await shot('1-start');

      report!(MakeStage.writing);
      p(const Narration('Sketching a focus timer with today’s log.'));
      p(const ToolStarted('write_file', 'Writing index.html'));
      p(const PartialFile('index.html', 'early'));
      await tester.pump(const Duration(seconds: 2));
      await shot('2-early');

      p(const PartialFile('index.html', 'mid'));
      await tester.pump(const Duration(seconds: 3));
      await shot('3-mid');

      p(const ToolStarted('render', 'Checking how it looks'));
      p(Inspected(pages['full']!, 1));
      await tester.pump(const Duration(seconds: 3));
      await shot('4-inspected');

      p(const ToolStarted('ask', 'Asking you a question'));
      await tester.pump(const Duration(seconds: 1));
      await shot('5-asking');

      done.complete('a1');
      await tester.pump();
    });
  }
}
