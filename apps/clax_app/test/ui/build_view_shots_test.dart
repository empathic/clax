// Renders the build view, the build chip and sheet over an app, and the
// launcher's building tile at phone size, light and dark, for a person to
// look at. Off by default; run with
//   CLAX_X1_SHOTS=1 flutter test --update-goldens test/ui/build_view_shots_test.dart
// after rendering the page images into target/flutter-app/u1/src (see
// making_screen_shots_test.dart), and look at target/flutter-app/x1/*.png.
import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/build_plan.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/making_preview.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/working_chip.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';
import 'build_view_test.dart' show FakeBuilds;

final _on = Platform.environment['CLAX_X1_SHOTS'] == '1';
const _out = '../../../../target/flutter-app/x1';
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

class _ShotPreview implements PreviewView {
  _ShotPreview(this.png);
  final Uint8List png;
  @override
  Future<void> loadHtml(String html) async {}
  @override
  Widget build(BuildContext context) => Image.memory(
    png,
    fit: BoxFit.fitWidth,
    alignment: Alignment.topCenter,
    gaplessPlayback: true,
  );
  @override
  void dispose() {}
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

const _features = [
  PlanFeature(
    id: 'f1',
    title: 'Start and stop a session',
    status: FeatureStatus.done,
  ),
  PlanFeature(id: 'f2', title: 'Today’s log', status: FeatureStatus.done),
  PlanFeature(
    id: 'f3',
    title: 'Weekly focus chart',
    detail: 'Bars for each day of the week',
    status: FeatureStatus.doing,
  ),
  PlanFeature(
    id: 'f4',
    title: 'Daily goal',
    detail: 'A target in minutes with a ring',
  ),
  PlanFeature(
    id: 'f5',
    title: 'Break reminders',
    detail: 'A notification after each session',
  ),
  PlanFeature(id: 'f6', title: 'Dark mode toggle', by: FeatureBy.person),
  PlanFeature(id: 'f7', title: 'Sounds', status: FeatureStatus.dropped),
];

void main() {
  for (final dark in [false, true]) {
    final scheme = dark ? 'dark' : 'light';

    Future<Uint8List> setUp(WidgetTester tester) async {
      late Uint8List png;
      await tester.runAsync(() async {
        await _fonts();
        png = await File('$_src/page-full-$scheme.png').readAsBytes();
      });
      tester.view.physicalSize = const Size(1080, 2340);
      tester.view.devicePixelRatio = 1080 / 412;
      addTearDown(tester.view.reset);
      return png;
    }

    Widget app(Widget home) => MaterialApp(
      debugShowCheckedModeBanner: false,
      theme: claxTheme(Brightness.light),
      darkTheme: claxTheme(Brightness.dark),
      themeMode: dark ? ThemeMode.dark : ThemeMode.light,
      home: home,
    );

    Future<void> shot(WidgetTester tester, String name) async {
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
        matchesGoldenFile('$_out/build-$name-$scheme.png'),
      );
    }

    testWidgets('build view, $scheme', skip: !_on, (tester) async {
      final png = await setUp(tester);
      final plans = PlanStore();
      final control = FakeBuilds();
      final link = BuildLink(plans: plans, control: control);
      final made = Completer<String>();
      late void Function(MakeUpdate) report;
      await tester.pumpWidget(
        app(
          MakingScreen(
            request: 'a focus timer with a log of today’s sessions',
            make: (_, _, r) {
              report = r;
              return made.future;
            },
            preview: () => _ShotPreview(png),
            build: link,
            onMade: (_, _, _) {},
          ),
        ),
      );
      report(const MakePlanKey('new:1'));
      report(MakeStage.writing);
      report(const MakeProgress(Narration('Writing the timer screen')));
      report(const MakeProgress(PartialFile('index.html', '<p>x</p>')));
      await plans.set('new:1', [
        for (final f in _features.take(5))
          f.id == 'f1'
              ? f.copyWith(status: FeatureStatus.doing)
              : f.copyWith(status: FeatureStatus.todo),
      ]);
      await shot(tester, '1-first-run');

      await plans.set('new:1', _features);
      await plans.move('new:1', 'a1');
      made.complete('a1');
      await tester.pump();
      control.emit(
        const BuildState(
          'a1',
          BuildPhase.running,
          featureId: 'f3',
          progress: Narration('Drawing a bar for each day'),
        ),
      );
      await shot(tester, '2-open-app');

      final sheet = find.byKey(const ValueKey('plan-headline'));
      await tester.drag(sheet, const Offset(0, -500));
      await shot(tester, '3-plan-open');
    });

    testWidgets('chip and sheet over an app, $scheme', skip: !_on, (
      tester,
    ) async {
      final png = await setUp(tester);
      final plans = PlanStore();
      final control = FakeBuilds();
      final link = BuildLink(plans: plans, control: control);
      final api = FakeClaxApi();
      final aid = api.seedArtifact('Focus', {'index.html': '<p>x</p>'});
      await plans.set(aid, _features);
      final surface = _PictureSurface(png);
      await tester.pumpWidget(
        app(
          AppScreen(
            aid: aid,
            base: Uri.parse('http://127.0.0.1:7999'),
            api: api,
            surface: surface,
            voice: FakeVoice.unsupported('no'),
            settings: AppSettings(),
            timings: _NoTimings(),
            worker: const Stream.empty(),
            build: link,
          ),
        ),
      );
      surface.push(
        Ready(artifact: aid, version: 2, latest: 2, file: 'index.html'),
      );
      control.emit(
        BuildState(
          aid,
          BuildPhase.running,
          featureId: 'f3',
          progress: const Narration('Drawing a bar for each day'),
        ),
      );
      await shot(tester, '4-chip');
      await tester.tap(find.byType(BuildChip));
      await shot(tester, '5-sheet');
    });

    testWidgets('launcher tile, $scheme', skip: !_on, (tester) async {
      await setUp(tester);
      final plans = PlanStore();
      final control = FakeBuilds();
      final link = BuildLink(plans: plans, control: control);
      var t = 0;
      final api = FakeClaxApi(
        clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
      );
      api.seedArtifact('Groceries', {'index.html': '<p>x</p>'});
      api.seedArtifact('Run log', {'index.html': '<p>x</p>'});
      final aid = api.seedArtifact('Focus timer', {'index.html': '<p>x</p>'});
      await plans.set(aid, _features);
      await tester.pumpWidget(
        app(
          Launcher(
            api: api,
            open: (_) {},
            make: (_, _, _) async => throw StateError('unused'),
            voice: FakeVoice.unsupported('no'),
            build: link,
          ),
        ),
      );
      control.emit(BuildState(aid, BuildPhase.running, featureId: 'f3'));
      await shot(tester, '6-launcher');
    });
  }
}

class _NoTimings implements Timings {
  @override
  void start(String name) {}
  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) =>
      null;
  @override
  List<TimingRecord> recent(int n) => const [];
}
