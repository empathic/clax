import 'dart:typed_data';
import 'dart:async';
import 'dart:convert';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/loop.dart' show MakeStopped;
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/scripted_backend.dart';
import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/queue/client.dart' hide Intent;
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/haiku.dart';
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/making_preview.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/fake_queue.dart';

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
      expect(previewAllows('about:srcdoc'), true);
      expect(previewSetsBackground(TargetPlatform.android), true);
      expect(previewSetsBackground(TargetPlatform.iOS), true);
      expect(
        previewSetsBackground(TargetPlatform.macOS),
        false,
        reason: 'WKWebView on macOS throws on setOpaque',
      );
      for (final u in [
        'data:text/html,x',
        'about:config',
        'https://example.com',
        'http://127.0.0.1:7480/',
        'file:///etc/passwd',
        'javascript:1',
      ]) {
        expect(previewAllows(u), false, reason: u);
      }
    },
  );

  group('makingTitle', () {
    test('quotes the request with the ellipsis inside the quotes', () {
      expect(
        makingTitle('A tide table for the harbour and the bay at dawn'),
        '“A tide table for…”',
      );
      expect(makingTitle('a timer'), '“a timer…”');
      expect(makingTitle('  a   timer.  '), '“a timer…”');
    });

    test('cuts a long request at a word boundary', () {
      const long =
          'a tracker for my runs with a weekly total and a monthly chart';
      final t = makingTitle(long);
      expect(t, startsWith('“'));
      expect(t, endsWith('…”'));
      final quoted = t.substring(1, t.length - 2);
      expect(quoted.length, lessThanOrEqualTo(makingTitleMax));
      expect(long.startsWith(quoted), isTrue);
      expect(long[quoted.length], ' ', reason: 'cut at a word boundary');
    });
  });

  _more();
  _screen();
  _plain();
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

/// A Keep going held open by the test.
class _KeepGoing {
  final keys = <String>[];
  void Function(MakeUpdate)? report;
  Completer<String> result = Completer<String>();

  Future<String> call(
    String request,
    String key,
    Future<void> cancelled,
    void Function(MakeUpdate) report,
  ) {
    keys.add(key);
    this.report = report;
    result = Completer<String>();
    return result.future;
  }
}

/// A make that stopped at the turn limit once, with [spent] of its $6.
MakeStopped _stop({double spent = 1.2, int stops = 1}) => MakeStopped(
  key: 'new:k1',
  limit: '14 turns',
  stops: stops,
  spentUsd: spent,
  notes: const [
    'Writing the modal editor',
    'I wrote the file list and a modal editor, but ran out of room before '
        'I could get a clean render.',
  ],
  changed: const [(path: 'index.html', added: 40, removed: 0)],
  reason: 'I wrote the file list and a modal editor, but ran out of room '
      'before I could get a clean render.',
  html: '<p>normal mode</p>',
);

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

    test("a tool's step names its file only for developer labels", () {
      final l = log0();
      l.apply(MakeProgress(toolStarted('write_file', {'path': 'index.html'})));
      l.apply(MakeProgress(toolStarted('write_file', {'path': 'app.js'})));
      l.apply(MakeProgress(toolStarted('render', {})));
      expect(l.line, 'Checking it works');
      expect(l.earlier, ['Writing the page'], reason: 'one step, not two');
      expect(l.lineFor(developer: true), 'Checking it works');
      expect(l.earlierFor(developer: true), [
        'Writing index.html',
        'Writing app.js',
      ]);
    });

    test('a refusal of the starting data is kept', () {
      final l = log0();
      expect(l.startingDataRefused, isNull);
      l.apply(const MakeProgress(StartingDataRefused('a1', 'quota full')));
      expect(l.startingDataRefused, 'quota full');
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
  final keep = _KeepGoing();
  final preview = _FakePreview();
  final timings = _Timings();
  final clock = _Clock();
  final results = <MakingResult?>[];
  var previews = 0;

  /// Whether the screen makes its own (platform) preview.
  var webView = false;

  /// Whether the preview factory throws.
  var throwingPreview = false;
  var size = const Size(400, 800);
  double textScale = 1;

  /// Developer tools, and comment mode's quiet.
  var developer = false;
  var quiet = false;
  List<String> haikus = const ['Old pond, a frog jumps'];

  /// Given to the screen as its onMade.
  void Function(BuildContext context, String aid, Uint8List? lastFrame)? onMade;

  Future<void> pump(WidgetTester tester) async {
    await tester.binding.setSurfaceSize(size);
    addTearDown(() => tester.binding.setSurfaceSize(null));
    await tester.pumpWidget(
      MaterialApp(
        builder: (context, child) => MediaQuery(
          data: MediaQuery.of(context)
              .copyWith(textScaler: TextScaler.linear(textScale)),
          child: DeveloperScope(
            notifier: ValueNotifier(developer),
            child: ClaxQuiet(quiet: quiet, child: child!),
          ),
        ),
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () async {
              results.add(
                await Navigator.of(context).push<MakingResult>(
                  MaterialPageRoute(
                    builder: (_) => MakingScreen(
                      request: 'a timer',
                      make: make.call,
                      keepGoing: keep.call,
                      preview: webView
                          ? null
                          : () {
                              previews++;
                              if (throwingPreview) {
                                throw UnimplementedError('no preview here');
                              }
                              return preview;
                            },
                      timings: timings,
                      clock: clock,
                      onMade: onMade,
                      haikus: () async => haikus,
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
  testWidgets('a make whose job waits for Claude says so, and its Cancel '
      'is a Cancel intent', (tester) async {
    final q = FakeQueueClient()..push([makeJob('j')]);
    final makes = MakeService(q);
    addTearDown(makes.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: MakingScreen(
          request: 'a timer',
          session: makes.sessionOf('j'),
          preview: _FakePreview.new,
          haikus: () async => const [],
        ),
      ),
    );
    await _settle(tester);
    expect(find.text('Waiting for Claude'), findsNothing);
    q.put(makeJob('j', waitingFor: WaitingFor.claude));
    await _settle(tester);
    expect(find.text('Waiting for Claude'), findsOneWidget);
    expect(find.byKey(const ValueKey('make-try-now')), findsNothing);
    q.put(makeJob('j'));
    await _settle(tester);
    // The line's switch lets the old words fade out.
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('Waiting for Claude'), findsNothing);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
    expect(q.sent, [const Cancel('j')]);
  });

  testWidgets('a make whose run waits for Claude offers Try now', (
    tester,
  ) async {
    var tries = 0;
    final makes = MakeService.local((r, cancelled, stage) {
      stage(MakeStage.offline);
      return Completer<String>().future;
    }, tryNow: () => tries++);
    addTearDown(makes.dispose);
    final session = makes.start('a timer');
    await tester.pumpWidget(
      MaterialApp(
        home: MakingScreen(
          request: 'a timer',
          session: session,
          preview: _FakePreview.new,
        ),
      ),
    );
    await _settle(tester);
    expect(find.text('Waiting for Claude'), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('make-try-now')));
    expect(tries, 1);
    session.cancel();
    await _settle(tester);
  });

  testWidgets('a stopped make\'s Keep going and a failed one\'s Retry are '
      'intents, and the view follows its job', (tester) async {
    final q = FakeQueueClient()..push([makeJob('j')]);
    final makes = MakeService(q);
    addTearDown(makes.dispose);
    await tester.pumpWidget(
      MaterialApp(
        home: MakingScreen(
          request: 'a timer',
          session: makes.sessionOf('j'),
          preview: _FakePreview.new,
          haikus: () async => const [],
        ),
      ),
    );
    await _settle(tester);
    q.put(makeJob('j', state: JobState.stopped));
    await _settle(tester);
    expect(find.byKey(const ValueKey('make-stopped')), findsOneWidget);
    await tester.tap(find.text('Keep going'));
    await _settle(tester);
    expect(q.sent, [const KeepGoing('j')]);
    q.put(makeJob('j'));
    await _settle(tester);
    expect(find.byKey(const ValueKey('make-stopped')), findsNothing);
    q.put(makeJob('j', state: JobState.failed, error: 'doc_moved'));
    await _settle(tester);
    expect(find.textContaining('A document changed'), findsOneWidget);
    await tester.tap(find.text('Retry'));
    await _settle(tester);
    expect(q.sent.last, const Retry('j'));
  });

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
    expect(find.text('Getting started'), findsOneWidget);
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
    expect(find.text('Waiting for you'), findsOneWidget);
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
    expect(find.text('Couldn’t make it'), findsOneWidget);
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

  testWidgets('a make that stops at a limit says it made a start and offers '
      'Keep going first, the agent\'s own words only under Details', (
    tester,
  ) async {
    final rig = _Rig();
    await rig.pump(tester);
    rig.make.result.completeError(_stop());
    await _settle(tester);
    expect(find.text('This one’s bigger than one go'), findsOneWidget);
    expect(find.text('Couldn’t make it'), findsNothing);
    expect(find.textContaining('ran out of room'), findsNothing);
    expect(find.text('Stopped once · \$1.20 of \$6 used'), findsOneWidget);
    final keep = tester.widget<ClaxButton>(
      find.widgetWithText(ClaxButton, 'Keep going'),
    );
    expect(keep.kind, ClaxButtonKind.primary);
    expect(keep.isDefault, isTrue);
    final over = tester.widget<ClaxButton>(
      find.widgetWithText(ClaxButton, 'Start over'),
    );
    expect(over.kind, isNot(ClaxButtonKind.primary));
    expect(find.widgetWithText(ClaxButton, 'Edit'), findsOneWidget);

    await tester.ensureVisible(find.byKey(const ValueKey('make-stop-details')));
    await tester.tap(find.byKey(const ValueKey('make-stop-details')));
    await tester.pump();
    expect(find.text('index.html'), findsOneWidget);
    expect(find.text('+40'), findsOneWidget);
    expect(find.textContaining('ran out of room'), findsOneWidget);
    expect(find.text('Writing the modal editor'), findsOneWidget);
    await tester.ensureVisible(find.text('Cancel'));
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });

  testWidgets('Keep going goes on from the kept draft, its page in the '
      'preview, and the app it makes is handed over', (tester) async {
    final rig = _Rig();
    await rig.pump(tester);
    rig.make.result.completeError(_stop());
    await _settle(tester);
    await tester.tap(find.text('Keep going'));
    await _settle(tester);
    expect(rig.keep.keys, ['new:k1']);
    expect(rig.make.calls, 1);
    expect(find.text('This one’s bigger than one go'), findsNothing);
    expect(find.byKey(const ValueKey('making-title')), findsOneWidget);
    rig.clock.advance(const Duration(seconds: 1));
    await tester.pump();
    expect(rig.preview.loads, ['<p>normal mode</p>']);
    rig.keep.result.complete('a1');
    await _settle(tester);
    expect(rig.results.single, isA<Made>().having((m) => m.aid, 'aid', 'a1'));
  });

  testWidgets('Start over makes the app again from the request, and Edit '
      'goes back to it', (tester) async {
    final rig = _Rig();
    await rig.pump(tester);
    rig.make.result.completeError(_stop());
    await _settle(tester);
    await tester.tap(find.text('Start over'));
    await _settle(tester);
    expect(rig.make.calls, 2);
    expect(rig.keep.keys, isEmpty);
    rig.make.result.completeError(_stop());
    await _settle(tester);
    await tester.ensureVisible(find.text('Edit'));
    await tester.tap(find.text('Edit'));
    await _settle(tester);
    expect(rig.results.single, isA<EditRequest>());
  });

  testWidgets('a make that has used its \$6 offers no Keep going', (
    tester,
  ) async {
    final rig = _Rig();
    await rig.pump(tester);
    rig.make.result.completeError(_stop(spent: 5.99, stops: 2));
    await _settle(tester);
    expect(find.text('This one’s bigger than one go'), findsOneWidget);
    expect(find.text('Keep going'), findsNothing);
    final over = tester.widget<ClaxButton>(
      find.widgetWithText(ClaxButton, 'Start over'),
    );
    expect(over.kind, ClaxButtonKind.primary);
    await tester.ensureVisible(find.text('Cancel'));
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

  testWidgets('a preview that cannot be made leaves the placeholder and the '
      'run going', (tester) async {
    final rig = _Rig()..throwingPreview = true;
    await rig.pump(tester);
    rig.progress(const PartialFile('index.html', '<p>hi'));
    rig.progress(const ToolStarted('render', 'Checking how it looks'));
    rig.progress(const PartialFile('index.html', '<p>hi there'));
    await tester.pump();
    expect(tester.takeException(), isNull);
    expect(find.text('Your app appears here as it is written'), findsOneWidget);
    expect(find.text('Checking how it looks'), findsOneWidget);
    rig.make.result.complete('a1');
    await _settle(tester);
    expect(rig.results.single, isA<Made>());
  });

  testWidgets('at text scale 2 on a small phone the failure view scrolls to '
      'Retry, Edit and Cancel', (tester) async {
    final rig = _Rig()
      ..size = const Size(360, 640)
      ..textScale = 2;
    await rig.pump(tester);
    rig.make.result.completeError(
      const AgentError(
        'The model refused this request. Check the model in Settings, or '
        'change the request and try again.',
      ),
    );
    await _settle(tester);
    expect(tester.takeException(), isNull);
    for (final label in ['Retry', 'Edit', 'Cancel']) {
      await tester.ensureVisible(find.text(label));
      await tester.pump();
      expect(find.text(label).hitTestable(), findsOneWidget, reason: label);
    }
    await tester.ensureVisible(find.text('Retry'));
    await tester.pump();
    await tester.tap(find.text('Retry'));
    await _settle(tester);
    expect(rig.make.calls, 2);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });
}

void _plain() {
  group('in a wide window', () {
    /// The make's screen as a pane's content (no route of its own) on
    /// [platform] in a [size] window; what it leaves with goes to
    /// [exits].
    Future<_Rig> pane(
      WidgetTester tester,
      List<MakingResult?> exits, {
      Size size = const Size(1200, 800),
      TargetPlatform platform = TargetPlatform.macOS,
    }) async {
      final rig = _Rig();
      tester.view.physicalSize = size;
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      await tester.pumpWidget(
        MaterialApp(
          theme: ThemeData(platform: platform),
          home: MakingScreen(
            request: 'a timer',
            make: rig.make.call,
            preview: () => rig.preview,
            clock: rig.clock,
            haikus: () async => const [],
            onExit: exits.add,
          ),
        ),
      );
      await _settle(tester);
      rig.progress(const PartialFile('index.html', '<p>hi'));
      await tester.pump();
      return rig;
    }

    testWidgets('at 1200 the preview is the page at the pane’s width, wider '
        'than a phone column and at most 960', (tester) async {
      await pane(tester, []);
      final card = tester.getSize(find.byKey(const ValueKey('fake-preview')));
      expect(card.width, greaterThan(448));
      expect(card.width, lessThanOrEqualTo(960));
    });

    testWidgets('on a phone the preview keeps to the phone’s width', (
      tester,
    ) async {
      final rig = _Rig();
      rig.size = const Size(412, 800);
      await rig.pump(tester);
      rig.progress(const PartialFile('index.html', '<p>hi'));
      await tester.pump();
      final card = tester.getSize(find.byKey(const ValueKey('fake-preview')));
      expect(card.width, lessThanOrEqualTo(448));
    });

    testWidgets('in a pane Back (and ⌘[, the navigator’s back) leaves the '
        'make going, through onExit, popping nothing', (tester) async {
      final exits = <MakingResult?>[];
      final rig = await pane(tester, exits);
      var cancelled = false;
      unawaited(rig.make.cancelled!.then((_) => cancelled = true));
      await tester.tap(find.byKey(const ValueKey('build-back')));
      await _settle(tester);
      expect(exits.single, isA<MakingLeft>());
      await tester.binding.handlePopRoute();
      await _settle(tester);
      expect(exits, hasLength(2));
      expect(exits.last, isA<MakingLeft>());
      expect(find.byType(MakingScreen), findsOneWidget);
      expect(cancelled, isFalse);
    });

    testWidgets('in a pane Cancel stops the make and exits with null; a made '
        'app exits with Made', (tester) async {
      final exits = <MakingResult?>[];
      var rig = await pane(tester, exits);
      await tester.tap(find.text('Cancel'));
      await _settle(tester);
      expect(exits, [null]);
      await tester.pumpWidget(const SizedBox());
      exits.clear();
      rig = await pane(tester, exits);
      rig.make.result.complete('a1');
      await _settle(tester);
      expect(exits.single, isA<Made>().having((m) => m.aid, 'aid', 'a1'));
    });
  });

  testWidgets('the title is the quoted request, left-aligned in title-1 on '
      'at most two lines', (tester) async {
    final rig = _Rig();
    await rig.pump(tester);
    final title = tester.widget<Text>(find.byKey(const ValueKey('making-title')));
    expect(title.data, '“a timer…”');
    expect(title.textAlign, TextAlign.start);
    expect(title.maxLines, 2);
    expect(title.style?.fontSize, 22);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });

  testWidgets('the phone closes with an X in the app bar, leaving the make', (
    tester,
  ) async {
    final rig = _Rig();
    await rig.pump(tester);
    expect(find.text('Back to apps'), findsNothing);
    final close = find.byKey(const ValueKey('build-back'));
    expect(
      find.descendant(of: find.byType(AppBar), matching: close),
      findsOneWidget,
    );
    expect(
      find.descendant(of: close, matching: find.byIcon(Icons.close_rounded)),
      findsOneWidget,
    );
    await tester.tap(close);
    await _settle(tester);
    expect(rig.results.single, isA<MakingLeft>());
  });

  testWidgets('no button under the making screen shrinks its tap target', (
    tester,
  ) async {
    final makes = MakeService.local((r, cancelled, stage) {
      stage(MakeStage.offline);
      return Completer<String>().future;
    }, tryNow: () {});
    addTearDown(makes.dispose);
    final session = makes.start('a timer');
    await tester.pumpWidget(
      MaterialApp(
        home: MakingScreen(
          request: 'a timer',
          session: session,
          preview: _FakePreview.new,
          haikus: () async => const [],
        ),
      ),
    );
    await _settle(tester);
    final buttons = find.descendant(
      of: find.byType(MakingScreen),
      matching: find.byWidgetPredicate((w) => w is ButtonStyleButton),
    );
    expect(buttons, findsWidgets);
    for (final e in buttons.evaluate()) {
      final b = e.widget as ButtonStyleButton;
      expect(
        b.style?.tapTargetSize,
        isNot(MaterialTapTargetSize.shrinkWrap),
        reason: '$b',
      );
    }
    expect(
      tester.getSize(find.byKey(const ValueKey('make-try-now'))).height,
      greaterThanOrEqualTo(48),
    );
    session.cancel();
    await _settle(tester);
  });

  testWidgets('the Live badge carries its dot inside it', (tester) async {
    final rig = _Rig();
    await rig.pump(tester);
    rig.progress(const PartialFile('index.html', '<p>hi'));
    await tester.pump();
    final dot = tester.getRect(find.byKey(const ValueKey('live-dot')));
    final badge = tester.getRect(
      find
          .ancestor(of: find.text('Live'), matching: find.byType(DecoratedBox))
          .first,
    );
    expect(badge.contains(dot.topLeft) && badge.contains(dot.bottomRight), true);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });

  testWidgets('a haiku shows under the step after 8 s of waiting', (
    tester,
  ) async {
    final rig = _Rig();
    await rig.pump(tester);
    await tester.pump(const Duration(seconds: 6));
    expect(find.byType(HaikuLine), findsNothing);
    await tester.pump(const Duration(seconds: 8));
    expect(find.byType(HaikuLine), findsOneWidget);
    expect(find.text('Old pond, a frog jumps'), findsOneWidget);
    // Not while Clax asks something.
    rig.progress(const ToolStarted('ask', 'Asking you a question'));
    await tester.pump();
    expect(find.byType(HaikuLine), findsNothing);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });

  testWidgets('no haiku in comment mode', (tester) async {
    final rig = _Rig()..quiet = true;
    await rig.pump(tester);
    await tester.pump(const Duration(seconds: 10));
    expect(find.byType(HaikuLine), findsNothing);
    await tester.tap(find.text('Cancel'));
    await _settle(tester);
  });

  for (final developer in [false, true]) {
    testWidgets('a step names its file only with Developer tools on '
        '(developer: $developer)', (tester) async {
      final rig = _Rig()..developer = developer;
      await rig.pump(tester);
      rig.progress(toolStarted('write_file', {'path': 'index.html'}));
      await tester.pump();
      expect(
        find.text(developer ? 'Writing index.html' : 'Writing the page'),
        findsOneWidget,
      );
      await tester.tap(find.text('Cancel'));
      await _settle(tester);
    });

    testWidgets('a failure shows its raw text under Details only with '
        'Developer tools on (developer: $developer)', (tester) async {
      final rig = _Rig()..developer = developer;
      await rig.pump(tester);
      rig.make.result.completeError(StateError('disk on fire'));
      await _settle(tester);
      expect(find.text('Couldn’t make it'), findsOneWidget);
      expect(find.text('Something went wrong. Try again.'), findsOneWidget);
      expect(find.textContaining('disk on fire'), findsNothing);
      final details = find.byKey(const ValueKey('make-failure-details'));
      if (!developer) {
        expect(details, findsNothing);
      } else {
        await tester.ensureVisible(details);
        await tester.tap(details);
        await tester.pump();
        expect(find.textContaining('disk on fire'), findsOneWidget);
      }
      await tester.ensureVisible(find.text('Cancel'));
      await tester.tap(find.text('Cancel'));
      await _settle(tester);
    });

    testWidgets('a new app whose starting data was refused opens under a '
        'banner with Details (developer: $developer)', (tester) async {
      final rig = _Rig()..developer = developer;
      await rig.pump(tester);
      rig.progress(const StartingDataRefused('a1', 'quota full'));
      rig.make.result.complete('a1');
      await _settle(tester);
      expect(rig.results, isEmpty, reason: 'the screen stays for the banner');
      expect(
        find.text('Some of its starting data couldn’t be added.'),
        findsOneWidget,
      );
      await tester.tap(find.byKey(const ValueKey('starting-data-details')));
      await _settle(tester);
      expect(
        find.textContaining('ask Clax to add it'),
        findsOneWidget,
      );
      expect(
        find.textContaining('quota full'),
        developer ? findsOneWidget : findsNothing,
      );
      await tester.tap(find.text('OK'));
      await _settle(tester);
      await tester.tap(find.byKey(const ValueKey('open-app')));
      await _settle(tester);
      expect(rig.results.single, isA<Made>().having((m) => m.aid, 'aid', 'a1'));
    });
  }
}
