import 'dart:typed_data';
import 'dart:async';
import 'dart:convert';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/scripted_backend.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/ui/making_preview.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

const _result = EditResult(
  reply: 'Made it.',
  title: 'Timer',
  files: [FileWrite(path: 'index.html', content: '<!doctype html>')],
);

const _request = AgentRequest(
  kind: RequestKind.newApp,
  model: 'claude-haiku-5-5',
  systemPrompt: 'system',
  userText: 'a timer',
);

void main() {
  test(
    'StagedBackend reports writing, then publishing, under its own name',
    () async {
      final stages = <MakeStage>[];
      final inner = ScriptedBackend([_result]);
      final b = StagedBackend(inner, stages.add);
      expect(b.name, 'scripted');
      final r = await b.run(_request);
      expect(r.title, 'Timer');
      expect(stages, [MakeStage.writing, MakeStage.publishing]);
      expect(inner.seen, hasLength(1));
    },
  );

  test('a second run is a retry', () async {
    final stages = <MakeStage>[];
    final b = StagedBackend(ScriptedBackend([_result, _result]), stages.add);
    await b.run(_request);
    await b.run(_request);
    expect(stages, [
      MakeStage.writing,
      MakeStage.publishing,
      MakeStage.retrying,
      MakeStage.publishing,
    ]);
  });

  test('a failed run reports no publishing and rethrows', () async {
    final stages = <MakeStage>[];
    final b = StagedBackend(
      ScriptedBackend([const AgentError('busy')]),
      stages.add,
    );
    await expectLater(b.run(_request), throwsA(isA<AgentError>()));
    expect(stages, [MakeStage.writing]);
  });

  test('every stage has words to show', () {
    for (final s in MakeStage.values) {
      expect(s.label, isNotEmpty);
    }
  });

  test(
    'the preview document shuts the network and keeps the doctype first',
    () {
      expect(
        previewDocument('<!DOCTYPE html><html><p>x'),
        '<!DOCTYPE html><meta http-equiv="Content-Security-Policy" '
        'content="$previewPolicy"><html><p>x',
      );
      expect(previewDocument('<p>x'), startsWith('<meta http-equiv'));
      expect(previewPolicy, startsWith("default-src 'none'"));
      expect(previewAllows('about:blank'), true);
      expect(previewAllows('data:text/html,x'), true);
      for (final u in [
        'https://example.com',
        'http://127.0.0.1:7480/',
        'file:///etc/passwd',
        'javascript:1',
      ]) {
        expect(previewAllows(u), false, reason: u);
      }
    },
  );

  _more();
  _screen();
}

/// A clock moved only by the test; each delay completes once the clock
/// reaches it.
class _Clock implements Clock {
  DateTime _now = DateTime.utc(2026, 10, 12, 9);
  final _timers = <(DateTime, Completer<void>)>[];

  @override
  DateTime now() => _now;

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _timers.add((_now.add(d), c));
    return c.future;
  }

  void advance(Duration d) {
    _now = _now.add(d);
    _timers.sort((a, b) => a.$1.compareTo(b.$1));
    while (_timers.isNotEmpty && !_timers.first.$1.isAfter(_now)) {
      _timers.removeAt(0).$2.complete();
    }
  }
}

class _FakePreview implements PreviewView {
  final loads = <String>[];
  var disposed = false;

  @override
  Future<void> loadHtml(String html) async => loads.add(html);

  @override
  Widget build(BuildContext context) =>
      Text('preview ${loads.length}', key: const ValueKey('fake-preview'));

  @override
  void dispose() => disposed = true;
}

class _Timings implements Timings {
  final started = <String>[];
  final stopped = <(String, Map<String, Object?>)>[];

  @override
  void start(String name) => started.add(name);

  @override
  TimingRecord? stop(String name, {Map<String, Object?> extra = const {}}) {
    stopped.add((name, extra));
    return null;
  }

  @override
  List<TimingRecord> recent(int n) => const [];
}

/// A make held open by the test, which reports through [report].
class _Make {
  void Function(MakeUpdate)? report;
  Future<void>? cancelled;
  var calls = 0;
  Completer<String> result = Completer<String>();

  Future<String> call(
    String request,
    Future<void> cancelled,
    void Function(MakeUpdate) report,
  ) {
    calls++;
    this.cancelled = cancelled;
    this.report = report;
    result = Completer<String>();
    return result.future;
  }

  void progress(AgentProgress e) => report!(MakeProgress(e));
}

final _png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4z8DwHwAFAAH/'
  'iZk9HQAAAABJRU5ErkJggg==',
);

void _more() {
  group('MakingLog', () {
    MakingLog log0() => MakingLog();

    test('the line is the latest step, the stage before any', () {
      final l = log0();
      expect(l.line, MakeStage.starting.label);
      l.apply(MakeStage.writing);
      expect(l.line, MakeStage.writing.label);
      l.apply(const MakeProgress(Narration('Sketching a timer.')));
      l.apply(
        const MakeProgress(ToolStarted('write_file', 'Writing index.html')),
      );
      expect(l.line, 'Writing index.html');
      expect(l.earlier, ['Sketching a timer.']);
      l.apply(const MakeProgress(Narration('a')));
      l.apply(const MakeProgress(Narration('b')));
      expect(l.line, 'b');
      expect(l.earlier, ['Writing index.html', 'a']);
      // A network cut shows over the steps.
      l.apply(MakeStage.paused);
      expect(l.line, MakeStage.paused.label);
    });

    test('only index.html feeds the preview; a render shows over it', () {
      final l = log0();
      expect(l.apply(const MakeProgress(PartialFile('app.css', 'x'))), false);
      expect(
        l.apply(const MakeProgress(PartialFile('index.html', '<p'))),
        true,
      );
      expect(l.html, '<p');
      l.apply(MakeProgress(Inspected(_png, 2)));
      expect((l.inspected, l.errors), (true, 2));
      l.apply(const MakeProgress(PartialEdit('index.html', 'y')));
      expect(l.inspected, true, reason: 'an edit cannot be previewed');
      l.apply(const MakeProgress(PartialFile('index.html', '<p>2')));
      expect((l.inspected, l.html), (false, '<p>2'));
    });

    test('a resend keeps the preview until its text catches up', () {
      final l = log0();
      l.apply(const MakeProgress(PartialFile('index.html', '<p>hello</p>')));
      l.apply(const MakeProgress(AttemptStarted(2)));
      expect(
        l.apply(const MakeProgress(PartialFile('index.html', '<p>'))),
        false,
      );
      expect(l.html, '<p>hello</p>');
      expect(
        l.apply(const MakeProgress(PartialFile('index.html', '<p>hello</p>!'))),
        true,
      );
      expect(l.html, '<p>hello</p>!');
      // A shorter final text shows once the next step begins.
      l.apply(const MakeProgress(AttemptStarted(3)));
      l.apply(const MakeProgress(PartialFile('index.html', '<b>')));
      expect(l.html, '<p>hello</p>!');
      expect(
        l.apply(
          const MakeProgress(ToolStarted('render', 'Checking how it looks')),
        ),
        true,
      );
      expect(l.html, '<b>');
    });

    test('after a render, a rewrite keeps the screenshot until it is as '
        'long as the page shown', () {
      final l = log0();
      l.apply(const MakeProgress(PartialFile('index.html', '<p>one</p>')));
      l.apply(MakeProgress(Inspected(_png, 1)));
      l.apply(
        const MakeProgress(ToolStarted('write_file', 'Writing index.html')),
      );
      expect(
        l.apply(const MakeProgress(PartialFile('index.html', '<p>'))),
        false,
      );
      expect((l.inspected, l.html), (true, '<p>one</p>'));
      expect(
        l.apply(const MakeProgress(PartialFile('index.html', '<p>one</p>!'))),
        true,
      );
      expect((l.inspected, l.html), (false, '<p>one</p>!'));
    });

    test('asking lasts until the next event', () {
      final l = log0();
      l.apply(const MakeProgress(ToolStarted('ask', 'Asking you a question')));
      expect(l.asking, true);
      l.apply(const MakeProgress(Narration('Thanks.')));
      expect(l.asking, false);
    });
  });

  group('PreviewThrottle', () {
    Future<void> turn() => Future<void>.delayed(Duration.zero);

    test('loads at most once per interval, always the newest text', () async {
      final clock = _Clock();
      final loads = <String>[];
      final t = PreviewThrottle(load: loads.add, clock: clock);
      t.offer('a');
      expect(loads, ['a'], reason: 'the first text shows at once');
      t.offer('ab');
      t.offer('abc');
      expect(loads, ['a']);
      clock.advance(const Duration(milliseconds: 249));
      await turn();
      expect(loads, ['a']);
      clock.advance(const Duration(milliseconds: 1));
      await turn();
      expect(loads, ['a', 'abc']);
      // A text every 50 ms for two seconds: four loads a second at most.
      loads.clear();
      for (var i = 0; i < 40; i++) {
        t.offer('x$i');
        clock.advance(const Duration(milliseconds: 50));
        await turn();
      }
      expect(loads.length, inInclusiveRange(7, 8));
      expect(loads.last, anyOf('x39', 'x38', 'x37', 'x36', 'x35'));
      clock.advance(const Duration(seconds: 1));
      await turn();
      expect(loads.last, 'x39', reason: 'the newest text always shows');
    });

    test('flush loads the waiting text now; close drops it', () async {
      final clock = _Clock();
      final loads = <String>[];
      final t = PreviewThrottle(load: loads.add, clock: clock);
      t.offer('a');
      t.offer('ab');
      t.flush();
      expect(loads, ['a', 'ab']);
      t.offer('abc');
      t.close();
      clock.advance(const Duration(seconds: 1));
      await Future<void>(() {});
      expect(loads, ['a', 'ab']);
    });
  });
}

/// Lets routes and switches finish; the progress bar never settles.
Future<void> _settle(WidgetTester tester) async {
  await tester.pump();
  await tester.pump(const Duration(seconds: 1));
}

class _Rig {
  final make = _Make();
  final preview = _FakePreview();
  final timings = _Timings();
  final clock = _Clock();
  final results = <MakingResult?>[];
  var previews = 0;

  /// Whether the screen makes its own (platform) preview.
  var webView = false;

  /// Given to the screen as its onMade.
  void Function(BuildContext context, String aid, Uint8List? lastFrame)? onMade;

  Future<void> pump(WidgetTester tester) async {
    await tester.binding.setSurfaceSize(const Size(400, 800));
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              results.add(
                await Navigator.of(context).push<MakingResult>(
                  MaterialPageRoute(
                    builder: (_) => MakingScreen(
                      request: 'a timer',
                      make: make.call,
                      preview: webView
                          ? null
                          : () {
                              previews++;
                              return preview;
                            },
                      timings: timings,
                      clock: clock,
                      onMade: onMade,
                    ),
                  ),
                ),
              );
            },
            child: const Text('go'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('go'));
    await _settle(tester);
  }

  void progress(AgentProgress e) => make.progress(e);
}

void _screen() {
  testWidgets('streamed index.html loads into the preview at most four '
      'times a second, and its first frame stops say-first-output', (
    tester,
  ) async {
    final rig = _Rig();
    await rig.pump(tester);
    expect(rig.timings.started, [sayFirstOutput]);
    expect(find.byKey(const ValueKey('fake-preview')), findsNothing);
    rig.progress(const ToolStarted('write_file', 'Writing index.html'));
    for (var i = 1; i <= 10; i++) {
      rig.progress(PartialFile('index.html', '<!doctype html><h1>T${'i' * i}'));
    }
    await tester.pump();
    expect(rig.preview.loads, hasLength(1));
    expect(rig.preview.loads.single, '<!doctype html><h1>Ti');
    expect(rig.timings.stopped.map((s) => s.$1), [sayFirstOutput]);
    expect(find.byKey(const ValueKey('fake-preview')), findsOneWidget);
    expect(find.text('Live'), findsOneWidget);
    expect(find.text('Writing index.html'), findsOneWidget);

    rig.clock.advance(const Duration(milliseconds: 250));
    await tester.pump();
    expect(rig.preview.loads, hasLength(2));
    expect(rig.preview.loads.last, contains('T${'i' * 10}'));
    expect(rig.previews, 1, reason: 'one view for the whole run');
    expect(rig.timings.stopped, hasLength(1));

    await tester.tap(find.text('Cancel'));
    await _settle(tester);
    expect(rig.results, [null]);
    expect(rig.preview.disposed, true);
  });

  testWidgets('a render shows its screenshot and its errors over the '
      'preview; writing the page again goes back to the preview', (
    tester,
  ) async {
    final rig = _Rig();
    await rig.pump(tester);
    rig.progress(const PartialFile('index.html', '<p>hi'));
    rig.progress(const ToolStarted('render', 'Checking how it looks'));
    rig.progress(Inspected(_png, 2));
    await tester.pump();
    expect(find.text('Checking how it looks'), findsOneWidget);
    expect(find.text('2 errors to fix'), findsOneWidget);
    final shot = find.byKey(const ValueKey('inspection'));
    expect(shot, findsOneWidget);
    double opacity() => tester
        .widget<AnimatedOpacity>(
          find.ancestor(of: shot, matching: find.byType(AnimatedOpacity)),
        )
        .opacity;
    expect(opacity(), 1);
    rig.progress(Inspected(_png, 0));
    await tester.pump();
    expect(find.text('Checked'), findsOneWidget);
    rig.progress(const PartialFile('index.html', '<p>hi again'));
    await tester.pump();
    expect(opacity(), 0);
    expect(find.text('Live'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });

  testWidgets('narration shows the latest step and the two before it', (
    tester,
  ) async {
    final rig = _Rig();
    await rig.pump(tester);
    expect(find.text('Starting the agent'), findsOneWidget);
    rig.progress(const Narration('Planning a timer.'));
    rig.progress(const ToolStarted('write_file', 'Writing index.html'));
    rig.progress(const ToolStarted('render', 'Checking how it looks'));
    rig.progress(const ToolStarted('finish', 'Publishing'));
    await _settle(tester);
    expect(find.text('Publishing'), findsOneWidget);
    expect(find.text('Writing index.html'), findsOneWidget);
    expect(find.text('Checking how it looks'), findsOneWidget);
    expect(find.text('Planning a timer.'), findsNothing);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });

  testWidgets('while the agent asks, the seconds stop', (tester) async {
    final rig = _Rig();
    await rig.pump(tester);
    await tester.pump(const Duration(seconds: 2));
    expect(find.text('3 s'), findsOneWidget);
    rig.progress(const ToolStarted('ask', 'Asking you a question'));
    await tester.pump(const Duration(seconds: 5));
    expect(find.text('waiting for you'), findsOneWidget);
    rig.progress(const Narration('Got it.'));
    await tester.pump(const Duration(seconds: 1));
    expect(find.text('4 s'), findsOneWidget);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });

  testWidgets('Cancel stops the run; a made app pops Made', (tester) async {
    final rig = _Rig();
    await rig.pump(tester);
    var cancelled = false;
    unawaited(rig.make.cancelled!.then((_) => cancelled = true));
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
    expect(cancelled, true);
    expect(rig.results, [null]);

    await tester.tap(find.text('go'));
    await _settle(tester);
    rig.progress(const PartialFile('index.html', '<p>hi'));
    await tester.pump();
    rig.make.result.complete('a1');
    await _settle(tester);
    expect(rig.make.calls, 2);
    expect(rig.results, hasLength(2));
    expect(rig.results.last, isA<Made>().having((m) => m.aid, 'aid', 'a1'));
  });

  testWidgets('with onMade the made app goes to it, with the last render, '
      'in place of the pop', (tester) async {
    final rig = _Rig();
    final handed = <(String, Uint8List?)>[];
    rig.onMade = (context, aid, frame) {
      handed.add((aid, frame));
      Navigator.of(context).pushReplacement(
        MaterialPageRoute<void>(builder: (_) => const Text('the app')),
        result: Made(aid),
      );
    };
    await rig.pump(tester);
    rig.progress(const PartialFile('index.html', '<p>hi'));
    rig.progress(Inspected(_png, 0));
    await tester.pump();
    rig.make.result.complete('a1');
    await _settle(tester);
    expect(handed, [('a1', _png)]);
    expect(find.text('the app'), findsOneWidget);
    expect(rig.results.single, isA<Made>().having((m) => m.aid, 'aid', 'a1'));
  });

  testWidgets('Retry starts a fresh attempt: no old preview, the timing '
      'restarted, events of the old attempt dropped', (tester) async {
    final rig = _Rig();
    await rig.pump(tester);
    rig.progress(const PartialFile('index.html', '<p>first'));
    rig.progress(const Narration('First try.'));
    await tester.pump();
    final old = rig.make.report!;
    rig.make.result.completeError(const AgentError('busy'));
    await _settle(tester);
    expect(find.text("Couldn't make it"), findsOneWidget);
    await tester.tap(find.text('Retry'));
    await _settle(tester);
    expect(rig.make.calls, 2);
    expect(rig.timings.started, [sayFirstOutput, sayFirstOutput]);
    expect(find.text('First try.'), findsNothing);
    expect(find.byKey(const ValueKey('fake-preview')), findsNothing);
    old(const MakeProgress(Narration('Stale.')));
    await tester.pump();
    expect(find.text('Stale.'), findsNothing);
    rig.progress(const PartialFile('index.html', '<p>second'));
    await tester.pump();
    expect(rig.timings.stopped, hasLength(2));
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });

  testWidgets('without a platform WebView the card keeps its placeholder', (
    tester,
  ) async {
    final rig = _Rig()..webView = true;
    await rig.pump(tester);
    rig.progress(const PartialFile('index.html', '<p>hi'));
    await tester.pump();
    expect(find.text('Your app appears here as it is written'), findsOneWidget);
    expect(find.text('Live'), findsNothing);
    expect(rig.timings.stopped, isEmpty);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });
}
