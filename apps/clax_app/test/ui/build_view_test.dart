import 'dart:async';

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
import 'package:clax_app/ui/making_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/working_chip.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

/// The worker's builds, driven by the test.
class FakeBuilds implements BuildControl {
  final _c = StreamController<BuildState>.broadcast(sync: true);
  final states = <String, BuildState>{};
  final calls = <String>[];

  void emit(BuildState s) {
    states[s.appId] = s;
    _c.add(s);
  }

  @override
  Stream<BuildState> get builds => _c.stream;

  @override
  BuildState? buildOf(String appId) => states[appId];

  @override
  Future<void> stopBuild(String appId) async {
    calls.add('stop $appId');
    emit(BuildState(appId, BuildPhase.stopped));
  }

  @override
  Future<void> resumeBuild(String appId) async {
    calls.add('resume $appId');
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

const _features = [
  PlanFeature(id: 'f1', title: 'Log a run', status: FeatureStatus.done),
  PlanFeature(
    id: 'f2',
    title: 'Weekly chart',
    detail: 'A bar chart of the week',
    status: FeatureStatus.doing,
  ),
  PlanFeature(id: 'f3', title: 'Export CSV', detail: 'Share the runs'),
  PlanFeature(id: 'f4', title: 'Goals'),
];

Future<void> frames(WidgetTester tester, [int n = 10]) async {
  for (var i = 0; i < n; i++) {
    await tester.pump(const Duration(milliseconds: 50));
  }
}

void phone(WidgetTester tester) {
  tester.view.devicePixelRatio = 3;
  tester.view.physicalSize = const Size(360, 780) * 3;
  addTearDown(tester.view.reset);
}

void main() {
  late PlanStore plans;
  late FakeBuilds control;
  late BuildLink link;

  setUp(() {
    plans = PlanStore();
    control = FakeBuilds();
    link = BuildLink(plans: plans, control: control);
  });

  group('the checklist', () {
    Future<void> pumpList(WidgetTester tester, String key) async {
      phone(tester);
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light),
          home: Scaffold(
            body: BuildWatch(
              link: link,
              appId: key,
              builder: (context, plan, state) => BuildPlanPanel(
                plan: plan,
                state: state,
                plans: plans,
                planKey: key,
              ),
            ),
          ),
        ),
      );
      await frames(tester);
    }

    testWidgets('shows each feature with what the agent does now', (
      tester,
    ) async {
      await plans.set('a1', _features);
      control.emit(
        const BuildState(
          'a1',
          BuildPhase.running,
          featureId: 'f2',
          progress: Narration('Drawing the bars'),
        ),
      );
      await pumpList(tester, 'a1');
      expect(find.text('Plan · 1 of 4 done'), findsOneWidget);
      expect(find.text('Building Weekly chart'), findsOneWidget);
      expect(find.byKey(const ValueKey('feature-narration')), findsOneWidget);
      expect(find.text('Drawing the bars'), findsOneWidget);
      expect(find.text('Share the runs'), findsOneWidget);
    });

    testWidgets('adds, edits, drops, takes a drop back and reorders', (
      tester,
    ) async {
      await plans.set('a1', _features);
      await pumpList(tester, 'a1');

      await tester.enterText(
        find.byKey(const ValueKey('add-feature-field')),
        'Dark mode',
      );
      await tester.tap(find.byKey(const ValueKey('add-feature-submit')));
      await frames(tester);
      var p = (await plans.get('a1'))!;
      expect(p.features.last.title, 'Dark mode');
      expect(p.features.last.by, FeatureBy.person);
      expect(find.text('Dark mode  you', findRichText: true), findsOneWidget);

      await tester.tap(find.text('Export CSV', findRichText: true));
      await frames(tester);
      await tester.enterText(
        find.byKey(const ValueKey('edit-feature-title')),
        'Export to CSV',
      );
      await tester.tap(find.byKey(const ValueKey('edit-feature-save')));
      await frames(tester);
      p = (await plans.get('a1'))!;
      expect(p.feature('f3')!.title, 'Export to CSV');

      await tester.drag(
        find.text('Goals', findRichText: true),
        const Offset(-500, 0),
      );
      await frames(tester);
      p = (await plans.get('a1'))!;
      expect(p.feature('f4')!.status, FeatureStatus.dropped);

      await tester.tap(find.text('Goals', findRichText: true));
      await frames(tester);
      p = (await plans.get('a1'))!;
      expect(p.feature('f4')!.status, FeatureStatus.todo);

      // Drag Goals' handle above Export to CSV.
      final g = await tester.startGesture(
        tester.getCenter(find.byKey(const ValueKey('reorder-f4'))),
      );
      for (var i = 0; i < 10; i++) {
        await g.moveBy(const Offset(0, -10));
        await tester.pump(const Duration(milliseconds: 20));
      }
      await g.up();
      await frames(tester);
      p = (await plans.get('a1'))!;
      expect([for (final f in p.features) f.id].indexOf('f4'), 2);
    });
  });

  group('the build view', () {
    Future<List<String>> pumpMaking(
      WidgetTester tester, {
      required Completer<String> made,
      required void Function(void Function(MakeUpdate) report) onStart,
    }) async {
      phone(tester);
      final handed = <String>[];
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light),
          home: MakingScreen(
            request: 'a run tracker',
            make: (_, _, report) {
              onStart(report);
              return made.future;
            },
            build: link,
            onMade: (_, aid, _) => handed.add(aid),
          ),
        ),
      );
      await frames(tester);
      return handed;
    }

    testWidgets('shows the plan from the first run, offers Open app once '
        'the first version lands, and follows the build', (tester) async {
      final made = Completer<String>();
      late void Function(MakeUpdate) report;
      final handed = await pumpMaking(
        tester,
        made: made,
        onStart: (r) => report = r,
      );
      expect(find.byKey(const ValueKey('plan-sheet')), findsNothing);

      report(const MakePlanKey('new:1'));
      await frames(tester);
      expect(find.text('Planning the features…'), findsOneWidget);

      await plans.set('new:1', _features);
      await frames(tester);
      expect(find.text('Plan · 1 of 4 done'), findsOneWidget);
      expect(find.text('Cancel'), findsOneWidget);

      await plans.move('new:1', 'a1');
      made.complete('a1');
      await frames(tester);
      expect(handed, isEmpty, reason: 'the build goes on in the build view');
      expect(find.byKey(const ValueKey('open-app')), findsOneWidget);
      expect(find.text('Cancel'), findsNothing);

      control.emit(
        const BuildState(
          'a1',
          BuildPhase.running,
          featureId: 'f2',
          progress: Narration('Drawing the bars'),
        ),
      );
      await frames(tester);
      expect(find.text('Building Weekly chart'), findsOneWidget);
      expect(find.text('Drawing the bars'), findsWidgets);
      expect(find.byKey(const ValueKey('build-stop')), findsOneWidget);

      await tester.tap(find.byKey(const ValueKey('open-app')));
      await frames(tester);
      expect(handed, ['a1']);
    });

    testWidgets('hands the app over at once when nothing is left to build', (
      tester,
    ) async {
      final made = Completer<String>();
      final handed = await pumpMaking(tester, made: made, onStart: (_) {});
      await plans.set('a1', [
        const PlanFeature(id: 'f1', title: 'Log', status: FeatureStatus.done),
      ]);
      made.complete('a1');
      await frames(tester);
      expect(handed, ['a1']);
    });
  });

  group('over the app', () {
    Future<(FakeAppSurface, String)> pumpApp(WidgetTester tester) async {
      phone(tester);
      final api = FakeClaxApi();
      final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
      final surface = FakeAppSurface();
      await plans.set(aid, _features);
      await tester.pumpWidget(
        MaterialApp(
          theme: claxTheme(Brightness.light),
          home: AppScreen(
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
        Ready(artifact: aid, version: 1, latest: 1, file: 'index.html'),
      );
      await frames(tester);
      return (surface, aid);
    }

    testWidgets('the chip shows the build while it goes; its sheet stops it '
        'and a toast says so', (tester) async {
      final (_, aid) = await pumpApp(tester);
      expect(find.byType(BuildChip), findsNothing);

      control.emit(BuildState(aid, BuildPhase.running, featureId: 'f2'));
      await frames(tester);
      expect(find.text('Building · Weekly chart · 1/4'), findsOneWidget);

      await tester.tap(find.byType(BuildChip));
      await frames(tester, 20);
      expect(find.byKey(const ValueKey('build-sheet')), findsOneWidget);
      expect(find.text('Plan · 1 of 4 done'), findsOneWidget);

      await tester.tap(find.byKey(const ValueKey('build-stop')));
      await frames(tester);
      expect(control.calls, ['stop $aid']);
      expect(find.byKey(const ValueKey('build-resume')), findsOneWidget);
      expect(find.text('Build stopped'), findsWidgets);
      // The sheet says so while it is open; the chip is gone under it.
      expect(find.byType(BuildChip), findsNothing);
    });

    testWidgets('a toast says the build is done', (tester) async {
      final (_, aid) = await pumpApp(tester);
      control.emit(BuildState(aid, BuildPhase.running, featureId: 'f2'));
      await frames(tester);
      control.emit(BuildState(aid, BuildPhase.done));
      await frames(tester);
      expect(find.byType(BuildChip), findsNothing);
      expect(find.text('Every feature is built'), findsOneWidget);
    });

    testWidgets('Add to plan makes the comment a feature and closes the '
        'composer without sending', (tester) async {
      final (surface, aid) = await pumpApp(tester);
      surface.push(
        const StateMsg(
          commenting: true,
          draft: Draft(
            label: 'h1',
            text: 'Add a dark mode',
            clip: DraftClip.ready,
          ),
          busy: false,
        ),
      );
      await frames(tester);
      await tester.tap(find.byKey(const ValueKey('add-to-plan')));
      await frames(tester);
      final p = (await plans.get(aid))!;
      expect(p.features.last.title, 'Add a dark mode');
      expect(p.features.last.by, FeatureBy.person);
      expect(surface.hostCalls, contains(isA<CancelDraft>()));
      expect(surface.hostCalls, isNot(contains(isA<SubmitDraft>())));
      expect(control.calls, ['resume $aid']);
      expect(find.text('Added “Add a dark mode” to the plan'), findsOneWidget);
    });
  });

  group('the launcher', () {
    testWidgets('a tile shows its going build, and Inspect shows the plan', (
      tester,
    ) async {
      phone(tester);
      final api = FakeClaxApi();
      final aid = api.seedArtifact('Runs', {'index.html': '<h1>Runs</h1>'});
      await plans.set(aid, _features);
      await tester.pumpWidget(
        MaterialApp(
          home: Launcher(
            api: api,
            open: (_) {},
            make: (_, _, _) async => throw StateError('unused'),
            voice: FakeVoice.unsupported('no'),
            build: link,
          ),
        ),
      );
      await frames(tester);
      expect(find.byKey(const ValueKey('tile-building')), findsNothing);

      control.emit(BuildState(aid, BuildPhase.running, featureId: 'f2'));
      await frames(tester);
      expect(find.byKey(const ValueKey('tile-building')), findsOneWidget);
      expect(find.text('1/4'), findsOneWidget);

      control.emit(BuildState(aid, BuildPhase.done));
      await frames(tester);
      expect(find.byKey(const ValueKey('tile-building')), findsNothing);

      await tester.longPress(find.text('Runs'));
      await frames(tester);
      expect(find.text('Inspect'), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('inspect')));
      await frames(tester, 20);
      await tester.ensureVisible(find.widgetWithText(Tab, 'Plan'));
      await frames(tester);
      await tester.tap(find.widgetWithText(Tab, 'Plan'));
      await frames(tester, 20);
      expect(find.text('Weekly chart', findRichText: true), findsOneWidget);
      expect(find.text('Plan · 1 of 4 done'), findsOneWidget);
      expect(find.byKey(const ValueKey('add-feature-field')), findsNothing);
    });
  });
}
