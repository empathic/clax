import 'dart:async';

import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/agent/tools/tool_spec.dart';
import 'package:clax_app/agent/tools/toolbox.dart';
import 'package:flutter_test/flutter_test.dart';

import 'support.dart';

/// A tool that ends the run, standing in for `finish`.
class _EndTool implements Tool {
  _EndTool(this.name);

  final String name;
  var calls = 0;

  @override
  ToolSpec get spec => ToolSpec(name, 'Ends the run.', const {
    'type': 'object',
    'properties': {},
    'required': [],
    'additionalProperties': false,
  });

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    calls++;
    s.outcome = const RunFinished(version: 4, answered: true);
    return const ToolOk({'published': 4});
  }
}

class _ThrowingTool implements Tool {
  @override
  ToolSpec get spec => const ToolSpec('give_up', 'Throws.', {
    'type': 'object',
    'properties': {},
    'required': [],
    'additionalProperties': false,
  });

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) =>
      throw const FormatException('the daemon said no');
}

/// A render that snapshots the draft, waits for [gate], then marks what
/// it saw as rendered clean.
class _GatedRender implements Tool {
  _GatedRender(this.gate);

  final Completer<void> gate;
  var started = 0;

  @override
  ToolSpec get spec => const ToolSpec('render', 'Renders.', {
    'type': 'object',
    'properties': {},
    'required': [],
    'additionalProperties': false,
  });

  @override
  Future<ToolOutput> call(Map<String, Object?> input, RunState s) async {
    started++;
    final seen = s.draft.snapshot();
    await gate.future;
    s.draft.markRendered(seen, errors: false);
    return const ToolOk({'console_errors': []});
  }
}

void main() {
  test('specs follow toolOrder and leave out missing tools', () {
    final tb = toolboxWith(extra: [_EndTool('finish')]);
    expect(tb.specs.map((s) => s.name), [
      'list_files',
      'read_file',
      'write_file',
      'str_replace',
      'delete_file',
      'set_meta',
      'finish',
    ]);
  });

  test('a tool outside toolOrder, or twice, is a programming error', () {
    expect(() => toolboxWith(extra: [_EndTool('shell')]), throwsArgumentError);
    expect(
      () => toolboxWith(extra: [_EndTool('finish'), _EndTool('finish')]),
      throwsArgumentError,
    );
  });

  test('an unknown tool is refused', () async {
    final r = await toolboxWith().call('shell', {});
    expect(r, isA<ToolRefused>());
    expect((r as ToolRefused).message, contains('shell'));
  });

  test('every call after the outcome is set is refused', () async {
    final end = _EndTool('finish');
    final tb = toolboxWith(extra: [end]);
    await tb.call('finish', {});
    expect(tb.state.ended, isTrue);
    for (final name in ['write_file', 'finish']) {
      final r = await tb.call(name, {'path': 'a.txt', 'content': 'x'});
      expect(r, isA<ToolRefused>());
      expect((r as ToolRefused).message, 'The run has ended.');
    }
    expect(end.calls, 1);
    expect(tb.state.draft.files.containsKey('a.txt'), isFalse);
  });

  test('a tool that throws an exception answers as a refusal', () async {
    final tb = toolboxWith(extra: [_ThrowingTool()]);
    final r = await tb.call('give_up', {});
    expect(r, isA<ToolRefused>());
    expect((r as ToolRefused).message, contains('the daemon said no'));
    expect(tb.state.ended, isFalse);
  });

  test('past the wall clock every call is refused, finish too', () async {
    final clock = StepClock();
    final end = _EndTool('finish');
    final tb = toolboxWith(clock: clock, extra: [end]);
    clock.advance(const Duration(seconds: 45));
    final r = await tb.call('write_file', {'path': 'a.txt', 'content': 'x'});
    expect(r, isA<ToolRefused>());
    expect((r as ToolRefused).message, contains('45 seconds'));
    expect(tb.state.draft.files.containsKey('a.txt'), isFalse);
    expect(await tb.call('finish', {}), isA<ToolRefused>());
    expect(end.calls, 0);
  });

  test(
    'a call made before the deadline runs, even if it starts after',
    () async {
      final clock = StepClock();
      final gate = Completer<void>();
      final render = _GatedRender(gate);
      final end = _EndTool('finish');
      final tb = toolboxWith(clock: clock, extra: [render, end]);
      final first = tb.call('render', {});
      final second = tb.call('finish', {});
      clock.advance(const Duration(seconds: 45));
      gate.complete();
      expect(await first, isA<ToolOk>());
      expect(await second, isA<ToolOk>());
      expect(end.calls, 1);
    },
  );

  test('calls run one at a time, in order: a write during a render waits, '
      'and the draft is not marked clean', () async {
    final gate = Completer<void>();
    final render = _GatedRender(gate);
    final tb = toolboxWith(files: {'index.html': '<p>ok</p>'}, extra: [render]);
    final d = tb.state.draft;
    final rendering = tb.call('render', {});
    final writing = tb.call('write_file', {
      'path': 'index.html',
      'content': '<p>broken',
    });
    await pumpEventQueue();
    expect(render.started, 1);
    expect(d.files['index.html'], '<p>ok</p>', reason: 'the write waits');
    gate.complete();
    await Future.wait([rendering, writing]);
    expect(d.files['index.html'], '<p>broken');
    expect(d.renderedClean, isFalse);
    expect(d.lastClean!.files, {'index.html': '<p>ok</p>'});
  });

  test('the tools of the turn that reached the turn cap still run', () async {
    final tb = toolboxWith(caps: testChangeCaps);
    for (var i = 0; i < 6; i++) {
      tb.state.budget.addTurn(const Usage(output: 10));
    }
    expect(tb.state.budget.hardStop, '6 turns');
    expect(
      await tb.call('write_file', {'path': 'a.txt', 'content': 'x'}),
      isA<ToolOk>(),
    );
  });

  group('stopAtCap', () {
    test('without a handler gives up naming the cap', () async {
      final tb = toolboxWith();
      final out = await tb.stopAtCap('6 turns');
      expect(
        out,
        isA<RunGaveUp>().having((o) => o.reason, 'reason', '6 turns'),
      );
      expect((out as RunGaveUp).answered, isFalse);
      expect(tb.state.outcome, same(out));
    });

    test('hands the cap and narration to the handler', () async {
      final tb = toolboxWith();
      final seen = <(String, String?)>[];
      tb.capHandler = (cap, narration) async {
        seen.add((cap, narration));
        return const RunFinished(version: 4, answered: true);
      };
      final out = await tb.stopAtCap('45 seconds', narration: 'Adding totals');
      expect(seen, [('45 seconds', 'Adding totals')]);
      expect(out, isA<RunFinished>());
      expect(tb.state.outcome, same(out));
    });

    test('keeps an outcome already set', () async {
      final tb = toolboxWith();
      tb.state.outcome = const RunCancelled();
      var called = false;
      tb.capHandler = (_, _) async {
        called = true;
        return const RunGaveUp('x', answered: true);
      };
      expect(await tb.stopAtCap('6 turns'), isA<RunCancelled>());
      expect(called, isFalse);
    });
  });

  group('draft', () {
    test('a clean render keeps the snapshot it rendered', () async {
      final tb = toolboxWith(files: {'index.html': '<p>a</p>'});
      final d = tb.state.draft;
      await tb.call('str_replace', {
        'path': 'index.html',
        'old_str': 'a',
        'new_str': 'b',
      });
      d.markRendered(d.snapshot(), errors: false);
      expect(d.renderedRevision, d.revision);
      expect(d.renderedClean, isTrue);
      await tb.call('str_replace', {
        'path': 'index.html',
        'old_str': 'b',
        'new_str': 'c',
      });
      expect(d.renderedClean, isFalse);
      expect(d.lastClean!.files, {'index.html': '<p>b</p>'});
      expect(d.lastClean!.revision, 1);
      expect(d.lastClean!.baseVersion, 3);
      d.markRendered(d.snapshot(), errors: true);
      expect(d.lastRenderHadErrors, isTrue);
      expect(d.renderedClean, isFalse);
      expect(d.lastClean!.files, {'index.html': '<p>b</p>'});
    });

    test('an edit between the snapshot and the mark is not rendered', () {
      final d = toolboxWith(files: {'index.html': '<p>a</p>'}).state.draft;
      final seen = d.snapshot();
      d.writeFile('index.html', '<p>broken');
      d.markRendered(seen, errors: false);
      expect(d.renderedRevision, seen.revision);
      expect(d.renderedClean, isFalse);
      expect(d.lastClean, same(seen));
      expect(d.lastClean!.files, {'index.html': '<p>a</p>'});
    });

    test('a reset clears lastClean, and a snapshot of the old base is never '
        'kept', () {
      final d = toolboxWith(files: {'index.html': 'v3'}).state.draft;
      d.writeFile('index.html', 'mine');
      final old = d.snapshot();
      d.markRendered(old, errors: false);
      expect(d.cleanOnBase, same(old));
      d.reset(files: {'index.html': 'v4'}, baseVersion: 4);
      expect(d.lastClean, isNull);
      expect(d.cleanOnBase, isNull);
      d.markRendered(old, errors: false);
      expect(d.lastClean, isNull, reason: 'rendered on version 3');
      expect(d.renderedClean, isFalse);
      final fresh = d.snapshot();
      d.markRendered(fresh, errors: false);
      expect(d.cleanOnBase, same(fresh));
      expect(fresh.baseVersion, 4);
    });

    test('changedFrom compares files, meta and staged writes', () {
      final tb = toolboxWith(files: {'index.html': 'x'}, title: 'T');
      final d = tb.state.draft;
      expect(d.changedFrom({'index.html': 'x'}, title: 'T'), isFalse);
      expect(d.changedFrom({'index.html': 'y'}, title: 'T'), isTrue);
      expect(d.changedFrom({'index.html': 'x'}, title: 'U'), isTrue);
      d.setMeta(capabilities: {'db': <String, Object?>{}});
      expect(d.changedFrom({'index.html': 'x'}, title: 'T'), isTrue);
      expect(
        d.changedFrom(
          {'index.html': 'x'},
          title: 'T',
          caps: {'db': <String, Object?>{}},
        ),
        isFalse,
      );
    });
  });

  group('toolLabel', () {
    test('says what the tool does in plain words', () {
      expect(toolLabel('write_file', {'path': 'index.html'}), 'Writing the page');
      expect(toolLabel('write_file', {}), 'Writing the page');
      expect(toolLabel('str_replace', {'path': 'app.js'}), 'Changing the page');
      expect(
        toolLabel('db_list', {'collection': 'data/runs'}),
        'Reading your runs',
      );
      expect(toolLabel('render', {}), 'Checking it works');
      expect(toolLabel('ask', {}), 'Asking you a question');
      expect(toolLabel('finish', {}), 'Publishing');
      expect(toolLabel('mystery', {}), 'Working');
    });

    test('names the file only for developer labels', () {
      expect(
        toolLabel('write_file', {'path': 'index.html'}, developer: true),
        'Writing index.html',
      );
      expect(toolLabel('write_file', {}, developer: true), 'Writing a file');
      expect(
        toolLabel('str_replace', {'path': 'app.js'}, developer: true),
        'Changing app.js',
      );
    });

    test('toolStarted carries both labels', () {
      final t = toolStarted('write_file', {'path': 'index.html'});
      expect(t.label, 'Writing the page');
      expect(t.detail, 'Writing index.html');
      expect(t.labelFor(developer: false), 'Writing the page');
      expect(t.labelFor(developer: true), 'Writing index.html');
      expect(toolStarted('render', {}).detail, 'Checking it works');
    });
  });

  test('a RunContext defaults to no threads, items or documents', () {
    final ctx = toolboxWith(kind: RequestKind.newApp).state.ctx;
    expect(ctx.threadIds, isEmpty);
    expect(ctx.items, isEmpty);
    expect(ctx.shownDocs, isEmpty);
    expect(ctx.artifactId, isNull);
  });
}
