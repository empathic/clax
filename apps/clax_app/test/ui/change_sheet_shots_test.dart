// Renders the change in progress over an app, mid-run with steps and a
// render, at phone and desktop widths, light and dark, for a person to look
// at. Off by default; run with
//   CLAX_X3_SHOTS=1 flutter test --update-goldens test/ui/change_sheet_shots_test.dart
// after rendering the page images into target/flutter-app/u1/src (see
// making_screen_shots_test.dart), and look at target/flutter-app/x3/*.png.
import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

final _on = Platform.environment['CLAX_X3_SHOTS'] == '1';
const _out = '../../../../target/flutter-app/x3';
const _src = '../../target/flutter-app/u1/src';

Future<void> _fonts() async {
  final root = Platform.environment['FLUTTER_ROOT']!;
  final dir = '$root/bin/cache/artifacts/material_fonts';
  Future<ByteData> f(String n) async =>
      ByteData.sublistView(await File('$dir/$n').readAsBytes());
  await (FontLoader('Roboto')
        ..addFont(f('Roboto-Regular.ttf'))
        ..addFont(f('Roboto-Medium.ttf'))
        ..addFont(f('Roboto-Bold.ttf')))
      .load();
  await (FontLoader(
    'MaterialIcons',
  )..addFont(f('MaterialIcons-Regular.otf'))).load();
}

/// An app surface that shows a picture of the page.
class _PictureSurface extends FakeAppSurface {
  _PictureSurface(this.png);
  final Uint8List png;
  @override
  Widget build(BuildContext context) => ColoredBox(
    color: Theme.of(context).colorScheme.surface,
    child: Image.memory(
      png,
      fit: BoxFit.fitWidth,
      alignment: Alignment.topCenter,
    ),
  );
}

class _Clock implements Clock {
  DateTime t = DateTime.utc(2026, 10, 13, 9);
  @override
  DateTime now() => t;
  final _delays = <Completer<void>>[];
  @override
  Future<void> delay(Duration d) =>
      (_delays..add(Completer<void>())).last.future;

  /// Ends every delay asked for so far.
  void lapse() {
    for (final c in List.of(_delays)) {
      c.complete();
    }
    _delays.clear();
  }
}

void main() {
  for (final dark in [false, true]) {
    final scheme = dark ? 'dark' : 'light';
    for (final (form, size) in [
      ('phone', const Size(412, 892)),
      ('desktop', const Size(1280, 800)),
    ]) {
      testWidgets('change sheet, $form, $scheme', skip: !_on, (tester) async {
        late Uint8List page;
        late Uint8List render;
        await tester.runAsync(() async {
          await _fonts();
          page = await File('$_src/page-full-$scheme.png').readAsBytes();
          render = await File('$_src/page-mid-$scheme.png').readAsBytes();
        });
        tester.view.physicalSize = size * 2.625;
        tester.view.devicePixelRatio = 2.625;
        tester.view.padding = form == 'phone'
            ? const FakeViewPadding(top: 24 * 2.625)
            : FakeViewPadding.zero;
        addTearDown(tester.view.reset);

        final api = FakeClaxApi();
        final aid = api.seedArtifact('Focus', {'index.html': '<p>x</p>'});
        final tid = api
            .sendComment(
              aid,
              'Make the session log show this week, with a total per day',
              author: 'Alex',
              clip: page,
            )
            .threadId;
        final worker = StreamController<WorkerEvent>.broadcast(sync: true);
        final clock = _Clock();
        final surface = _PictureSurface(page);
        await tester.pumpWidget(
          MaterialApp(
            debugShowCheckedModeBanner: false,
            theme: claxTheme(Brightness.light, defaultTargetPlatform),
            darkTheme: claxTheme(Brightness.dark, defaultTargetPlatform),
            themeMode: dark ? ThemeMode.dark : ThemeMode.light,
            home: AppScreen(
              aid: aid,
              base: Uri.parse('http://127.0.0.1:7999'),
              api: api,
              surface: surface,
              voice: FakeVoice.unsupported('no'),
              settings: AppSettings(),
              timings: const NoTimings(),
              worker: worker.stream,
              clock: clock,
              onStop: (_) {},
            ),
          ),
        );
        surface.push(
          Ready(artifact: aid, version: 2, latest: 2, file: 'index.html'),
        );
        surface.push(Sent(tid));
        worker.add(WorkerWorking(aid, [tid]));
        for (final s in [
          'Reading the session log',
          'Changing index.html',
          'Grouping the sessions by day',
          'Checking how it looks',
        ]) {
          worker.add(WorkerProgress(aid, [tid], Narration(s)));
        }
        worker.add(WorkerProgress(aid, [tid], Inspected(render, 0)));
        clock.t = clock.t.add(const Duration(seconds: 47));
        await tester.pump(const Duration(seconds: 1));
        Future<void> shot(String name) async {
          for (var i = 0; i < 8; i++) {
            await tester.pump(const Duration(milliseconds: 100));
          }
          await tester.runAsync(() async {
            for (final e in find.byType(Image).evaluate()) {
              await precacheImage((e.widget as Image).image, e);
            }
          });
          await tester.pump(const Duration(milliseconds: 400));
          await expectLater(
            find.byType(MaterialApp),
            matchesGoldenFile('$_out/change-$name-$form-$scheme.png'),
          );
        }

        await shot('1-chip');
        clock.lapse();
        await shot('2-pill');
        await tester.tap(find.byKey(const ValueKey('chip-working-pill')));
        await tester.pump();
        await tester.tap(find.byKey(const ValueKey('chip-working')));
        await shot('3-sheet');
        await worker.close();
      });
    }
  }
}
