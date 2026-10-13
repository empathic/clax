import 'dart:io';

import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:flutter_test/flutter_test.dart';

import 'tools/support.dart';

Map<String, Object?> feature(
  String id, {
  String status = 'todo',
  String title = 'A feature',
}) => {'id': id, 'title': title, 'detail': 'It does a thing.', 'status': status};

void main() {
  group('PlanStore', () {
    test('keeps the person\'s edits in order and survives a restart', () async {
      final dir = await Directory.systemTemp.createTemp('plans');
      addTearDown(() => dir.delete(recursive: true));
      final file = File('${dir.path}/plans.json');
      final a = PlanStore(file: file, clock: StepClock());
      await a.set('app1', const [
        PlanFeature(id: 'f1', title: 'List'),
        PlanFeature(id: 'f2', title: 'Totals'),
      ], request: 'a run tracker');
      final added = await a.addFeature('app1', title: 'Export', detail: 'CSV');
      await a.editFeature('app1', 'f2', title: 'Weekly totals');
      await a.reorder('app1', [added.id, 'f1']);
      await a.drop('app1', 'f1');
      await a.spend('app1', 0.25);

      final p = (await PlanStore(file: file).get('app1'))!;
      expect(p.request, 'a run tracker');
      expect([for (final f in p.features) f.id], [added.id, 'f1', 'f2']);
      expect(p.feature('f2')!.title, 'Weekly totals');
      expect(p.feature('f1')!.status, FeatureStatus.dropped);
      expect([for (final f in p.todo) f.id], [added.id, 'f2']);
      expect(p.spentUsd, 0.25);
      expect(p.revision, 6);
    });

    test('watch gives the plan now and after each change; move carries it '
        'to the app\'s ID', () async {
      final s = PlanStore();
      final seen = <List<String>?>[];
      final sub = s
          .watch('app1')
          .listen((p) => seen.add(p?.features.map((f) => f.id).toList()));
      await pumpEventQueue();
      await s.set('new:1', const [PlanFeature(id: 'f1', title: 'List')]);
      await s.move('new:1', 'app1');
      await s.mark('app1', 'f1', FeatureStatus.done);
      await pumpEventQueue();
      expect(seen, [
        null,
        ['f1'],
        ['f1'],
      ]);
      expect(await s.get('new:1'), isNull);
      await sub.cancel();
    });

    test('stopping a build puts its doing feature back to todo', () async {
      final s = PlanStore();
      await s.set('a', const [
        PlanFeature(id: 'f1', title: 'x', status: FeatureStatus.doing),
      ]);
      final p = (await s.setStopped('a', true))!;
      expect(p.stopped, isTrue);
      expect(p.features.single.status, FeatureStatus.todo);
    });
  });

  group('plan tools', () {
    late PlanStore store;
    late RunPlan plan;
    late Toolbox tb;

    setUp(() {
      store = PlanStore();
      plan = RunPlan(store, 'app1', request: 'a run tracker');
      tb = toolboxWith(extra: [PlanSetTool(plan), PlanMarkTool(plan)]);
    });

    test('plan_set saves the whole list with the request; plan_mark marks '
        'one feature and names the next', () async {
      final r = await tb.call('plan_set', {
        'features': [feature('f1'), feature('f2')],
      });
      expect(r, isA<ToolOk>());
      final m = await tb.call('plan_mark', {'id': 'f1', 'status': 'done'});
      expect((m as ToolOk).json['next'], 'f2');
      final p = (await store.get('app1'))!;
      expect(p.request, 'a run tracker');
      expect(p.feature('f1')!.status, FeatureStatus.done);
      // The run saw its own writes: a revision goes through.
      expect(
        await tb.call('plan_set', {
          'features': [feature('f1', status: 'done'), feature('f3')],
        }),
        isA<ToolOk>(),
      );
    });

    test('plan_set is refused once the person changed the plan, with the '
        'plan as it is now, and goes through the second time', () async {
      await tb.call('plan_set', {
        'features': [feature('f1')],
      });
      await store.addFeature('app1', title: 'Dark charts');
      final r = await tb.call('plan_set', {
        'features': [feature('f1'), feature('f2')],
      });
      expect(r, isA<ToolRefused>());
      expect((r as ToolRefused).message, contains('Dark charts'));
      expect(
        await tb.call('plan_set', {
          'features': [feature('f1'), feature('p2'), feature('f2')],
        }),
        isA<ToolOk>(),
      );
    });

    test('bad input is refused', () async {
      expect(
        await tb.call('plan_set', {'features': <Object?>[]}),
        isA<ToolRefused>(),
      );
      expect(
        await tb.call('plan_set', {
          'features': [feature('f1'), feature('f1')],
        }),
        isA<ToolRefused>(),
      );
      expect(
        await tb.call('plan_mark', {'id': 'f9', 'status': 'done'}),
        isA<ToolRefused>(),
      );
      await tb.call('plan_set', {
        'features': [feature('f1')],
      });
      expect(
        await tb.call('plan_mark', {'id': 'f1', 'status': 'finished'}),
        isA<ToolRefused>(),
      );
    });
  });
}
