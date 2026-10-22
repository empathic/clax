import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/prompt.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart';
import 'package:clax_app/app.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/core/core_client.dart';
import 'package:clax_app/platform/background_work.dart';
import 'package:clax_app/platform/connection.dart';
import 'package:clax_app/platform/pending_make.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/queue/client.dart';
import 'package:clax_app/queue/effect.dart' as q;
import 'package:clax_app/queue/host.dart';
import 'package:clax_app/queue/msg.dart' as q show Msg, OutOfTime, Woke;
import 'package:clax_app/ui/render_service.dart';
import 'package:clax_app/ui/app_cover.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart' show Ready;
import 'package:clax_app/ui/making_screen.dart' show MakingScreen;
import 'package:clax_app/ui/questions/question_card.dart';
import 'package:clax_app/ui/questions/questions_sheet.dart';
import 'package:clax_app/ui/read_consent_dialog.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'agent/loop_support.dart' show RecordingTimings, change, edit;
import 'agent/tools/support.dart' show StepClock;
import 'support/fake_inspector.dart';
import 'support/fake_queue.dart';
import 'support/fake_core.dart';
import 'support/fake_surface.dart';
import 'support/fake_voice.dart';

const _page = '<!doctype html><title>Runs</title><h1>Runs</h1>';

const _made = EditResult(
  reply: 'Made a run tracker.',
  title: 'Runs',
  files: [FileWrite(path: 'index.html', content: _page)],
  edits: [],
  removed: [],
  db: [],
  capabilities: {'db': <String, Object?>{}},
  resolve: false,
);

const _dns = AgentError(
  "Couldn't reach the Claude API: SocketException: Failed host lookup: "
  "'api.anthropic.com' (OS Error: No address associated with hostname, "
  'errno = 7)',
  retryable: true,
  kind: AgentErrorKind.network,
);

class _Key implements SecureKey {
  @override
  Future<String?> read() async => 'sk-ant-x';

  @override
  Future<void> write(String? key) async {}
}

/// Runs each request through the next step of [steps]: what it returns is
/// the result, what it throws is the failure.
class _Agent implements AgentBackend {
  final steps = <Future<EditResult> Function(AgentRequest r)>[];
  int runs = 0;

  @override
  String get name => backendMessagesApi;

  @override
  Future<EditResult> run(AgentRequest request) => steps[runs++](request);
}

/// The Messages API on the tool loop: each turn runs the next of [steps].
class _Turns implements AgentBackend, TurnBackend {
  final steps = <Future<ModelTurn> Function(TurnRequest r)>[];
  int turns = 0;

  @override
  String get name => backendMessagesApi;

  @override
  Future<EditResult> run(AgentRequest request) =>
      throw StateError('the tool loop sends turns');

  /// Runs the next step, then reports a [ToolStarted] for each of its
  /// tool calls, as a streamed turn does.
  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    final t = await steps[turns++](r);
    for (final b in t.content) {
      if (b case {
        'name': final String name,
        'input': final Map<String, Object?> input,
      }) {
        r.onProgress?.call(toolStarted(name, input));
      }
    }
    return t;
  }
}

/// A model turn that calls [calls], each `(name, input)`.
ModelTurn _calls(List<(String, Map<String, Object?>)> calls) => ModelTurn(
  content: [
    for (final (i, (name, input)) in calls.indexed)
      {'type': 'tool_use', 'id': 'call-$i', 'name': name, 'input': input},
  ],
  stopReason: 'tool_use',
  usage: const Usage(input: 100, output: 50),
);

/// The questions the questions sheet shows, in order.
List<String> _cards(WidgetTester tester) => [
  for (final c in tester.widgetList<QuestionCard>(find.byType(QuestionCard)))
    c.question.id,
];

/// Records every [BackgroundWork] call as `method(args…)`, and plays the
/// platform's launch artifact and taps.
class _Work implements BackgroundWork {
  final calls = <String>[];
  String? launch;
  final _opened = StreamController<String>.broadcast();

  final _stopped = StreamController<void>.broadcast();

  void tap(String aid) => _opened.add(aid);

  /// The platform says the service stopped (Android) or that the runs
  /// must pause now (iOS, [pausesRuns]).
  void stopNow() => _stopped.add(null);

  @override
  Stream<void> get stopped => _stopped.stream;

  @override
  Future<bool> start(String title, String step) async {
    calls.add('start($title, $step)');
    return true;
  }

  @override
  Future<void> update(String title, String step) async =>
      calls.add('update($title, $step)');

  @override
  Future<void> stop() async => calls.add('stop');

  /// What each news notification opens when tapped.
  final targets = <String>[];

  @override
  Future<void> ready(String title, String text, String artifactId) async {
    calls.add('ready($title, $text)');
    targets.add(artifactId);
  }

  @override
  Future<void> failed(
    String title,
    String reason, {
    String? artifactId,
  }) async => calls.add('failed($title, $reason)');

  @override
  Future<bool> requestPermission() async {
    calls.add('requestPermission');
    return true;
  }

  @override
  Future<String?> launchArtifact() async => launch;

  @override
  Stream<String> get opened => _opened.stream;

  @override
  bool pausesRuns = false;

  @override
  Future<void> paused(String title, String? artifactId) async =>
      calls.add('paused($title, $artifactId)');

  @override
  Future<void> pauseDone() async => calls.add('pauseDone');

  @override
  Future<void> clearPaused() async => calls.add('clearPaused');
}

void main() {
  late Directory dir;
  late FakeCoreHost coreHost;
  late FakeClaxApi api;
  late _Agent agent;
  late _Work work;
  late List<FakeAppSurface> surfaces;
  _Turns? turns;
  late StepClock clock;
  late bool reachable;

  setUp(() {
    turns = null;
    clock = StepClock();
    reachable = true;
    dir = Directory.systemTemp.createTempSync('app-bg-test-');
    coreHost = FakeCoreHost();
    api = FakeClaxApi();
    agent = _Agent();
    work = _Work();
    surfaces = [];
  });
  tearDown(() => dir.deleteSync(recursive: true));

  /// Pumps the app in an Android phone's window, where a screen opens over
  /// the home screen (a wider one lays out as a split window).
  Future<void> pumpApp(WidgetTester tester, {QueueStarter? startQueue}) async {
    tester.view.physicalSize = const Size(412, 915);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      AppRoot(
        services: AppServices(
          core: coreHost,
          api: (_) => api,
          backend: (_, _) => turns ?? agent,
          voice: FakeVoice.unsupported('No speech here'),
          settings: SettingsStore(File('${dir.path}/settings.json')),
          key: _Key(),
          timings: RecordingTimings(),
          agentTimings: RecordingTimings(),
          surface: () {
            final s = FakeAppSurface();
            surfaces.add(s);
            return s;
          },
          installId: 'install',
          supportDir: dir.path,
          prompts: PromptBuilder(rootBundle),
          background: work,
          pendingMake: PendingMake(File('${dir.path}/pending-make.json')),
          pid: 42,
          inspector: turns == null && startQueue == null
              ? null
              : FakeInspector(),
          startQueue: startQueue,
          consentMemory: FileConsentMemory(
            File('${dir.path}/agent-read-consent.json'),
          ),
          clock: clock,
          connection: ApiConnection(probe: () async => reachable, clock: clock),
        ),
      ),
    );
    await tester.pump();
    coreHost.set(const CoreReady());
    // Not pumpAndSettle: an app page's cover spins until its shell is ready.
    for (var i = 0; i < 5; i++) {
      await tester.pump(const Duration(milliseconds: 200));
    }
  }

  /// Pumps until [condition] holds; fails after [frames] frames without it.
  Future<void> pumpUntil(
    WidgetTester tester,
    bool Function() condition, {
    int frames = 200,
  }) async {
    for (var i = 0; i < frames; i++) {
      if (condition()) return;
      await tester.pump(const Duration(milliseconds: 10));
    }
    fail('condition not reached: $condition (calls: ${work.calls})');
  }

  /// Pumps until [text] shows, letting the app's real file reads finish
  /// (they do not settle in fake time).
  Future<void> pumpUntilShown(WidgetTester tester, String text) async {
    for (var i = 0; i < 200; i++) {
      if (find.text(text).evaluate().isNotEmpty) return;
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 10)),
      );
      await tester.pump(const Duration(milliseconds: 10));
    }
    fail('"$text" never showed');
  }

  /// Lets the app's queued file writes finish in real time.
  Future<void> settleFiles(WidgetTester tester) async {
    for (var i = 0; i < 60; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 10)),
      );
      await tester.pump();
    }
  }

  Future<void> pumpRoute(WidgetTester tester) async {
    for (var i = 0; i < 5; i++) {
      await tester.pump(const Duration(milliseconds: 200));
    }
  }

  void leave(WidgetTester tester) {
    for (final s in [
      AppLifecycleState.inactive,
      AppLifecycleState.hidden,
      AppLifecycleState.paused,
    ]) {
      tester.binding.handleAppLifecycleStateChanged(s);
    }
  }

  void comeBack(WidgetTester tester) {
    for (final s in [
      AppLifecycleState.hidden,
      AppLifecycleState.inactive,
      AppLifecycleState.resumed,
    ]) {
      tester.binding.handleAppLifecycleStateChanged(s);
    }
  }

  Future<void> submit(WidgetTester tester, String request) async {
    await tester.tap(find.text('New app'));
    await tester.pumpAndSettle();
    await tester.enterText(find.byType(TextField), request);
    await tester.pump();
    await tester.tap(find.text('Make it'));
    await tester.pump();
  }

  const title = 'Making “a tracker for my runs…”';

  testWidgets('a make in the foreground runs the service with its stages '
      'and posts nothing; permission is asked at the make', (tester) async {
    await pumpApp(tester);
    expect(work.calls, isEmpty, reason: 'nothing is asked at launch');
    agent.steps.add((_) async => _made);
    await submit(tester, 'a tracker for my runs');
    await pumpRoute(tester);
    expect(find.byType(AppScreen), findsOneWidget);
    await pumpUntil(tester, () => work.calls.contains('stop'));
    expect(work.calls, [
      'requestPermission',
      'start($title, Getting started)',
      'update($title, Writing your app)',
      'update($title, Checking and publishing)',
      'stop',
    ]);
  });

  testWidgets('a network cut in the background waits for the app to come '
      'back, runs again, and a make that lands in the background says it is '
      'ready', (tester) async {
    await pumpApp(tester);
    final gate = Completer<void>();
    agent.steps
      ..add((_) async {
        leave(tester);
        throw _dns;
      })
      ..add((_) async {
        await gate.future;
        return _made;
      });
    await submit(tester, 'a tracker for my runs');
    await pumpUntil(tester, () => work.calls.last.contains('Waiting for Clax'));
    expect(agent.runs, 1, reason: 'no run while the app is away');

    comeBack(tester);
    await tester.pump();
    expect(find.text('Picking up where it left off'), findsOneWidget);
    expect(find.text('Couldn’t make it'), findsNothing);
    await pumpUntil(tester, () => agent.runs == 2);

    leave(tester);
    gate.complete();
    await pumpUntil(
      tester,
      () => work.calls.contains('ready(Runs is ready, Tap to open it)'),
    );
    expect(work.calls, [
      'requestPermission',
      'start($title, Getting started)',
      'update($title, Writing your app)',
      'update($title, Waiting for Clax to come back…)',
      'update($title, Picking up where it left off)',
      'update($title, Checking and publishing)',
      'stop',
      'ready(Runs is ready, Tap to open it)',
    ]);
    comeBack(tester);
    await pumpRoute(tester);
    expect(find.byType(AppScreen), findsOneWidget);
  });

  testWidgets('a make that cannot reach the API in front waits for the '
      'connection, kept as waiting, and makes the app once it is back', (
    tester,
  ) async {
    await pumpApp(tester);
    reachable = false;
    agent.steps
      ..add((_) async => throw _dns)
      ..add((_) async => _made);
    await submit(tester, 'a tracker for my runs');
    await pumpRoute(tester);
    expect(find.text('Waiting for Claude'), findsOneWidget);
    expect(find.text('Couldn’t make it'), findsNothing);
    expect(agent.runs, 1);
    expect(work.calls.last, 'update($title, Waiting for Claude)');
    await settleFiles(tester);
    expect(
      File('${dir.path}/pending-make.json').readAsStringSync(),
      contains('"waiting":["a tracker for my runs"]'),
    );

    reachable = true;
    clock.advance(ApiConnection.maxProbe);
    await pumpRoute(tester);
    await pumpRoute(tester);
    expect(agent.runs, 2);
    expect(find.byType(AppScreen), findsOneWidget);
    expect(work.calls.where((c) => c.startsWith('failed')), isEmpty);
  });

  testWidgets('a make that waited for the connection when the app ended is '
      'started again by itself at the next launch', (tester) async {
    File('${dir.path}/pending-make.json').writeAsStringSync(
      '{"requests": ["a tracker for my runs"], '
      '"waiting": ["a tracker for my runs"]}',
    );
    agent.steps.add((_) async => _made);
    await pumpApp(tester);
    await settleFiles(tester);
    await pumpRoute(tester);
    expect(agent.runs, 1, reason: 'no tap needed');
    expect(find.text('Interrupted'), findsNothing);
    expect(find.text('Runs'), findsOneWidget);
  });

  testWidgets('a failure in front shows on the make; in the background a '
      'failure is posted', (tester) async {
    await pumpApp(tester);
    agent.steps.add(
      (_) async => throw const AgentError('The model is overloaded'),
    );
    await submit(tester, 'a tracker for my runs');
    await pumpRoute(tester);
    expect(find.text('Couldn’t make it'), findsOneWidget);
    expect(work.calls.last, 'stop');
    expect(work.calls.where((c) => c.startsWith('failed')), isEmpty);

    agent.steps.add((_) async {
      leave(tester);
      throw const AgentError('The API key was refused');
    });
    await tester.tap(find.text('Retry'));
    await pumpUntil(tester, () => work.calls.last.startsWith('failed'));
    expect(
      work.calls.last,
      'failed(Couldn’t make your app, The API key was refused)',
    );
  });

  testWidgets('a tapped notice opens its app once; a notice that launched '
      'the app opens it when the daemon is up', (tester) async {
    final aid = api.seedArtifact('Runs', {'index.html': _page});
    work.launch = aid;
    await pumpApp(tester);
    await pumpRoute(tester);
    expect(find.byType(AppScreen), findsOneWidget);

    work.tap(aid);
    await pumpRoute(tester);
    expect(surfaces, hasLength(1), reason: 'already on top');

    final other = api.seedArtifact('Laps', {'index.html': _page});
    work.tap(other);
    await pumpRoute(tester);
    expect(surfaces, hasLength(2));
  });

  testWidgets('a question asked in the background is posted; its tap shows '
      'the question sheet', (tester) async {
    final aid = api.seedArtifact('Runs', {'index.html': _page});
    await pumpApp(tester);
    leave(tester);
    final session = await api.registerAppSession(installId: 'i', pid: 1);
    final asked = await api.askQuestion(session.id, [
      {
        'question': 'Which unit should distances use?',
        'header': 'Units',
        'options': [
          {'label': 'km', 'description': ''},
          {'label': 'mi', 'description': ''},
        ],
      },
    ], artifactId: aid);
    await pumpUntil(tester, () => work.targets.isNotEmpty);
    expect(work.calls, [
      'ready(Runs has a question, Which unit should distances use?)',
    ]);
    expect(find.byType(QuestionsSheetView), findsNothing);
    comeBack(tester);
    work.tap(work.targets.single);
    await pumpRoute(tester);
    expect(find.byType(QuestionsSheetView), findsOneWidget);
    expect(surfaces, isEmpty, reason: 'the sheet, not the app');
    expect(work.targets.single, 'question:${asked.question.id}');
  });

  testWidgets('coming back without tapping the notification shows a question '
      'still open in the sheet; one answered meanwhile is not shown', (
    tester,
  ) async {
    await pumpApp(tester);
    leave(tester);
    final session = await api.registerAppSession(installId: 'i', pid: 1);
    Future<String> ask() async => (await api.askQuestion(session.id, [
      {
        'question': 'Which unit should distances use?',
        'header': 'Units',
        'options': [
          {'label': 'km', 'description': ''},
          {'label': 'mi', 'description': ''},
        ],
      },
    ])).question.id;
    final answered = await ask();
    final open = await ask();
    await pumpUntil(tester, () => work.targets.length == 2);
    await api.answerQuestion(answered, [
      const QuestionAnswer(selected: ['km']).toJson(),
    ]);
    await tester.pump();
    expect(find.byType(QuestionsSheetView), findsNothing);
    comeBack(tester);
    await pumpRoute(tester);
    expect(find.byType(QuestionsSheetView), findsOneWidget);
    expect(_cards(tester), [open]);
    // The notification's tap names the question on show: no second sheet.
    work.tap(work.targets.last);
    await pumpRoute(tester);
    expect(find.byType(QuestionsSheetView), findsOneWidget);
    await tester.binding.handlePopRoute();
    await pumpRoute(tester);
    expect(find.byType(QuestionsSheetView), findsNothing);
  });

  testWidgets('a background question about another app is told in a toast '
      'on return, not a sheet over the app', (tester) async {
    final aid = api.seedArtifact('Runs', {'index.html': _page});
    final other = api.seedArtifact('Log', {'index.html': _page});
    await pumpApp(tester);
    work.tap(aid);
    await pumpRoute(tester);
    expect(find.byType(AppScreen), findsOneWidget);
    leave(tester);
    final session = await api.registerAppSession(installId: 'i', pid: 1);
    await api.askQuestion(session.id, [
      {
        'question': 'Which unit should distances use?',
        'header': 'Units',
        'options': [
          {'label': 'km', 'description': ''},
          {'label': 'mi', 'description': ''},
        ],
      },
    ], artifactId: other);
    await pumpUntil(tester, () => work.targets.isNotEmpty);
    comeBack(tester);
    await tester.pump();
    expect(find.byType(QuestionsSheetView), findsNothing);
    expect(find.text('Log has a question'), findsOneWidget);
  });

  Future<String> askUnits({String? about}) async {
    final session = await api.registerAppSession(installId: 'i', pid: 1);
    return (await api.askQuestion(session.id, [
      {
        'question': 'Which unit should distances use?',
        'header': 'Units',
        'options': [
          {'label': 'km', 'description': ''},
          {'label': 'mi', 'description': ''},
        ],
      },
    ], artifactId: about)).question.id;
  }

  testWidgets('a background question the daemon withdrew without a stream '
      'event is not shown on return', (tester) async {
    await pumpApp(tester);
    leave(tester);
    final qid = await askUnits();
    await pumpUntil(tester, () => work.targets.isNotEmpty);
    api.withdrawUnannounced(qid);
    comeBack(tester);
    await pumpRoute(tester);
    expect(find.byType(QuestionsSheetView), findsNothing);
  });

  testWidgets("a tapped question notification shows its question first, the "
      'other background question about no app with it', (tester) async {
    await pumpApp(tester);
    leave(tester);
    final a = await askUnits();
    final b = await askUnits();
    await pumpUntil(tester, () => work.targets.length == 2);
    comeBack(tester);
    work.tap('question:$b');
    await pumpRoute(tester);
    expect(_cards(tester), [b, a]);
    await tester.binding.handlePopRoute();
    await pumpRoute(tester);
    expect(find.byType(QuestionsSheetView), findsNothing);
  });

  testWidgets('several background questions about other apps are told in one '
      'toast on return; its View shows each in turn', (tester) async {
    final aid = api.seedArtifact('Runs', {'index.html': _page});
    final log = api.seedArtifact('Log', {'index.html': _page});
    final diary = api.seedArtifact('Diary', {'index.html': _page});
    await pumpApp(tester);
    work.tap(aid);
    await pumpRoute(tester);
    leave(tester);
    final first = await askUnits(about: log);
    final second = await askUnits(about: diary);
    await pumpUntil(tester, () => work.targets.length == 2);
    comeBack(tester);
    await pumpRoute(tester);
    expect(find.byType(QuestionsSheetView), findsNothing);
    expect(find.text('Clax has 2 questions'), findsOneWidget);
    expect(find.text('Diary has a question'), findsNothing);
    await tester.tap(find.text('View'));
    await pumpRoute(tester);
    expect(_cards(tester), [first]);
    await tester.binding.handlePopRoute();
    await pumpRoute(tester);
    expect(_cards(tester), [second]);
  });

  testWidgets('a change that lands in the background says so', (tester) async {
    final aid = api.seedArtifact('Runs', {'index.html': _page});
    await pumpApp(tester);
    await pumpUntil(tester, () => api.waitingPolls == 1);
    final gate = Completer<void>();
    agent.steps.add((r) async {
      r.onProgress?.call(const OutputTokens(10));
      await gate.future;
      return change(edits: [edit('<h1>Runs</h1>', '<h1>My runs</h1>')]);
    });
    api.sendComment(aid, 'Call it My runs');
    await pumpUntil(tester, () => agent.runs == 1);
    leave(tester);
    gate.complete();
    await pumpUntil(
      tester,
      () => work.calls.contains('ready(Runs changed, Tap to see it)'),
    );
    expect(work.calls, [
      'start(Changing your app, Working on your comment)',
      'update(Changing Runs, Working on your comment)',
      'stop',
      'ready(Runs changed, Tap to see it)',
    ]);
  });

  group('when the platform pauses the runs', () {
    late String aid;
    late Completer<void> gate;

    /// A change on Groceries in flight in the background: its first run
    /// streams until it is cancelled or [gate] opens; a second run lands.
    Future<void> changeAway(WidgetTester tester) async {
      aid = api.seedArtifact('Groceries', {'index.html': _page});
      await pumpApp(tester);
      await pumpUntil(tester, () => api.waitingPolls == 1);
      gate = Completer<void>();
      agent.steps
        ..add((r) async {
          await Future.any([?r.cancelled, gate.future]);
          if (gate.isCompleted) {
            return change(edits: [edit('<h1>Runs</h1>', '<h1>My runs</h1>')]);
          }
          throw const AgentError.cancelled();
        })
        ..add(
          (_) async =>
              change(edits: [edit('<h1>Runs</h1>', '<h1>My runs</h1>')]),
        );
      api.sendComment(aid, 'Call it My runs');
      await pumpUntil(tester, () => agent.runs == 1);
      await pumpUntil(
        tester,
        () => work.calls.contains(
          'update(Changing Groceries, Working on your comment)',
        ),
      );
      leave(tester);
    }

    testWidgets('on iOS the change is paused with one notice, then '
        'pauseDone; on return the notice goes and the change lands', (
      tester,
    ) async {
      work.pausesRuns = true;
      await changeAway(tester);
      work.stopNow();
      await pumpUntil(tester, () => work.calls.contains('pauseDone'));
      final pauses = work.calls.where((c) => c.startsWith('paused')).toList();
      expect(pauses, ['paused(Groceries, $aid)']);
      expect(
        work.calls.indexOf('pauseDone'),
        greaterThan(work.calls.indexOf(pauses.single)),
      );
      expect(agent.runs, 1, reason: 'held while the app is away');

      comeBack(tester);
      await pumpUntil(tester, () => api.currentVersion(aid) == 2);
      expect(work.calls, contains('clearPaused'));
      expect(agent.runs, 2);
      expect(work.calls.where((c) => c.startsWith('pauseDone')), hasLength(1));
      expect(work.calls.where((c) => c.startsWith('failed')), isEmpty);
    });

    testWidgets('elsewhere the same stop pauses nothing', (tester) async {
      await changeAway(tester);
      work.stopNow();
      await tester.pump(const Duration(milliseconds: 50));
      gate.complete();
      await pumpUntil(tester, () => api.currentVersion(aid) == 2);
      expect(agent.runs, 1, reason: 'the run was not aborted');
      expect(work.calls.where((c) => c.startsWith('paused')), isEmpty);
      expect(work.calls, isNot(contains('pauseDone')));
    });
  });

  testWidgets('a cancelled make stops the service and posts nothing, even '
      'in the background', (tester) async {
    await pumpApp(tester);
    final gate = Completer<void>();
    agent.steps.add((r) async {
      await Future.any([gate.future, r.cancelled!]);
      throw const AgentError.cancelled();
    });
    await submit(tester, 'a tracker for my runs');
    await pumpUntil(tester, () => agent.runs == 1);
    await pumpRoute(tester);
    await settleFiles(tester);
    expect(
      File('${dir.path}/pending-make.json').readAsStringSync(),
      contains('a tracker for my runs'),
      reason: 'the make in flight is kept',
    );
    leave(tester);
    await tester.tap(find.text('Cancel'));
    await pumpUntil(tester, () => work.calls.contains('stop'));
    await pumpRoute(tester);
    expect(work.calls.last, 'stop');
    expect(work.calls.where((c) => c.startsWith('failed')), isEmpty);
    await settleFiles(tester);
    expect(File('${dir.path}/pending-make.json').existsSync(), isFalse);
  });

  testWidgets('a failed make stops the service', (tester) async {
    await pumpApp(tester);
    agent.steps.add((_) async => throw const AgentError('Claude refused'));
    await submit(tester, 'a tracker for my runs');
    await pumpRoute(tester);
    expect(find.text('Couldn’t make it'), findsOneWidget);
    expect(work.calls.last, 'stop');
    await settleFiles(tester);
    expect(File('${dir.path}/pending-make.json').existsSync(), isFalse);
  });

  testWidgets('a change cut off by quitting stops the service and posts '
      'nothing', (tester) async {
    final aid = api.seedArtifact('Runs', {'index.html': _page});
    await pumpApp(tester);
    await pumpUntil(tester, () => api.waitingPolls == 1);
    agent.steps.add((r) async {
      await r.cancelled;
      throw const AgentError.cancelled();
    });
    api.sendComment(aid, 'Call it My runs');
    await pumpUntil(tester, () => agent.runs == 1);
    leave(tester);
    await tester.binding.handleRequestAppExit();
    await pumpUntil(tester, () => work.calls.contains('stop'));
    expect(work.calls.where((c) => c.startsWith('ready')), isEmpty);
    expect(work.calls.where((c) => c.startsWith('failed')), isEmpty);
  });

  testWidgets('an interrupted change stops the service and posts nothing', (
    tester,
  ) async {
    final aid = api.seedArtifact('Runs', {'index.html': _page});
    await pumpApp(tester);
    await pumpUntil(tester, () => api.waitingPolls == 1);
    agent.steps
      ..add(
        (_) async => change(edits: [edit('<h1>Runs</h1>', '<h1>My runs</h1>')]),
      )
      // The worker runs an interrupted change again; that run never ends.
      ..add((_) => Completer<EditResult>().future);
    api.failNext(
      'publish',
      const ClaxApiError(
        status: 0,
        code: ClaxApiError.unreachable,
        message: 'no answer',
      ),
    );
    leave(tester);
    api.sendComment(aid, 'Call it My runs');
    await pumpUntil(tester, () => work.calls.contains('stop'));
    await tester.pump(const Duration(milliseconds: 50));
    expect(work.calls.where((c) => c.startsWith('ready')), isEmpty);
    expect(work.calls.where((c) => c.startsWith('failed')), isEmpty);
  });

  testWidgets('a make the last run never finished is an interrupted tile; '
      'Make it again runs it', (tester) async {
    // Seeded synchronously: real file futures do not settle in a widget
    // test's body.
    File('${dir.path}/pending-make.json')
        .writeAsStringSync('{"request": "a tracker for my runs"}');
    agent.steps.add((_) async => _made);
    await pumpApp(tester);
    await pumpUntilShown(tester, 'Interrupted');
    expect(find.text('This wasn’t finished.'), findsNothing, reason: 'no dialog');
    await settleFiles(tester);
    expect(
      File('${dir.path}/pending-make.json').existsSync(),
      isTrue,
      reason: 'kept until the person chooses',
    );
    await tester.tap(find.text('a tracker for my runs'));
    await pumpUntilShown(tester, 'Make it again');
    expect(find.text('This wasn’t finished.'), findsOneWidget);
    expect(find.text('“a tracker for my runs”'), findsOneWidget);
    await tester.tap(find.text('Make it again'));
    await pumpRoute(tester);
    await pumpRoute(tester);
    expect(agent.runs, 1);
    expect(find.byType(AppScreen), findsOneWidget);
    await settleFiles(tester);
    expect(File('${dir.path}/pending-make.json').existsSync(), isFalse);
  });

  testWidgets('Discard forgets the lost make; closing the question keeps it', (
    tester,
  ) async {
    File('${dir.path}/pending-make.json')
        .writeAsStringSync('{"request": "laps"}');
    await pumpApp(tester);
    await pumpUntilShown(tester, 'Interrupted');
    await tester.tap(find.text('laps'));
    await pumpUntilShown(tester, 'Discard');
    await tester.tapAt(const Offset(4, 4));
    await pumpRoute(tester);
    expect(find.text('Interrupted'), findsOneWidget);
    await tester.tap(find.text('laps'));
    await pumpUntilShown(tester, 'Discard');
    await tester.sendKeyEvent(LogicalKeyboardKey.escape);
    await pumpRoute(tester);
    expect(find.text('This wasn’t finished.'), findsNothing);
    expect(find.text('Interrupted'), findsOneWidget);
    await tester.tap(find.text('laps'));
    await pumpUntilShown(tester, 'Discard');
    await tester.tap(find.text('Discard'));
    await pumpRoute(tester);
    expect(find.text('Interrupted'), findsNothing);
    expect(agent.runs, 0);
    await settleFiles(tester);
    expect(File('${dir.path}/pending-make.json').existsSync(), isFalse);
  });

  group('on the tool loop', () {
    const page = '<!doctype html><title>Runs</title><h1>Runs</h1>';
    final make = [
      (
        'set_meta',
        <String, Object?>{'title': 'Runs', 'capabilities_json': null},
      ),
      ('write_file', <String, Object?>{'path': 'index.html', 'content': page}),
      ('render', <String, Object?>{'file': null}),
    ];
    const done = [
      (
        'write_file',
        <String, Object?>{
          'path': 'icon.svg',
          'content':
              '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
              '<rect width="1024" height="1024" fill="#2a6"/></svg>',
        },
      ),
      ('finish', <String, Object?>{'reply': 'Made it.', 'resolve': false}),
    ];

    testWidgets('a make shows its narration and tools in the notification, '
        'then opens the app', (tester) async {
      turns = _Turns();
      await pumpApp(tester);
      turns!.steps
        ..add((r) async {
          r.onProgress?.call(const Narration('Writing the tracker'));
          return _calls(make);
        })
        ..add((_) async => _calls(done));
      await submit(tester, 'a tracker for my runs');
      await pumpUntil(tester, () => work.calls.contains('stop'));
      await pumpRoute(tester);
      expect(find.byType(AppScreen), findsOneWidget);
      expect(work.calls, [
        'requestPermission',
        'start($title, Getting started)',
        'update($title, Writing the tracker)',
        'update($title, Naming the app)',
        'update($title, Writing the page)',
        'update($title, Checking it works)',
        'update($title, Writing the page)',
        'update($title, Publishing)',
        'stop',
      ]);
      final created = api.calls.singleWhere((c) => c.name == 'createArtifact');
      expect((created.args['request']! as PublishRequest).title, 'Runs');
      expect(await api.listDrafts(), isEmpty);
    });

    testWidgets('a make streams to the making screen, which hands over to '
        'the new app in its place, under its loading cover until ready', (
      tester,
    ) async {
      turns = _Turns();
      await pumpApp(tester);
      final gate = Completer<void>();
      turns!.steps
        ..add((r) async {
          r.onProgress?.call(const Narration('Laying out the tracker'));
          await gate.future;
          return _calls(make);
        })
        ..add((_) async => _calls(done));
      await submit(tester, 'a tracker for my runs');
      await pumpRoute(tester);
      final making = tester.state<State<MakingScreen>>(
        find.byType(MakingScreen),
      );
      expect(
        find.text('Laying out the tracker'),
        findsOneWidget,
        reason: "the run's progress reaches the making screen",
      );
      final makingRoute = ModalRoute.of(making.context)!;
      gate.complete();
      await pumpUntil(tester, () => work.calls.contains('stop'));
      await pumpRoute(tester);
      expect(find.byType(MakingScreen), findsNothing);
      expect(find.byType(AppScreen), findsOneWidget);
      expect(makingRoute.isActive, isFalse);
      // The app's route took the making route's place: one pop goes home.
      // No image of the last render: the app's own loading cover.
      expect(find.byKey(const ValueKey('app-cover-mark')), findsOneWidget);
      final cover = find.byKey(const ValueKey('app-cover'));
      expect(cover, findsOneWidget);
      final aid = (await api.listArtifacts()).single.id;
      final screen = tester.state(find.byType(AppScreen));
      final surface = surfaces.last;
      surface.push(
        Ready(artifact: aid, version: 1, latest: 1, file: 'index.html'),
      );
      // The mark showed while the route came in: it finishes its breath.
      await tester.pump();
      expect(cover, findsOneWidget);
      await tester.pump(AppCover.markHold);
      expect(cover, findsNothing);
      // The same screen and page stay, uncovered by the ready they saw.
      expect(tester.state(find.byType(AppScreen)), same(screen));
      expect(surface.disposed, isFalse);
      await tester.binding.handlePopRoute();
      await pumpRoute(tester);
      expect(find.byType(AppScreen), findsNothing);
      expect(find.text('New app'), findsOneWidget);
    });

    testWidgets('a turn cut by the background network block waits for the app '
        'and is sent again; the run goes on', (tester) async {
      turns = _Turns();
      await pumpApp(tester);
      final seen = <TurnRequest>[];
      turns!.steps
        ..add((r) async {
          seen.add(r);
          return _calls(make);
        })
        ..add((r) async {
          seen.add(r);
          leave(tester);
          throw _dns;
        })
        ..add((r) async {
          seen.add(r);
          return _calls(done);
        });
      await submit(tester, 'a tracker for my runs');
      await pumpUntil(
        tester,
        () => work.calls.last.contains('Waiting for Clax'),
      );
      expect(turns!.turns, 2, reason: 'held while the app is away');
      comeBack(tester);
      await pumpUntil(tester, () => work.calls.contains('stop'));
      expect(turns!.turns, 3);
      expect(seen[2].messages, seen[1].messages, reason: 'the same turn');
      expect(
        work.calls,
        containsAllInOrder([
          'update($title, Waiting for Clax to come back…)',
          'update($title, Picking up where it left off)',
          'stop',
        ]),
      );
      expect(api.callNames, contains('createArtifact'));
    });

    testWidgets('a make cancelled while its turn is held ends its waiting '
        'state with the run', (tester) async {
      turns = _Turns();
      await pumpApp(tester);
      // The make runs from Make on; the app goes away once its build view
      // is up.
      final shown = Completer<void>();
      turns!.steps.add((r) async {
        await shown.future;
        leave(tester);
        throw _dns;
      });
      await submit(tester, 'a tracker for my runs');
      await pumpUntilShown(tester, 'Cancel');
      shown.complete();
      await pumpUntil(
        tester,
        () => work.calls.last.contains('Waiting for Clax'),
      );
      await pumpRoute(tester);
      await tester.tap(find.text('Cancel'));
      await pumpUntil(tester, () => work.calls.contains('stop'));
      await pumpRoute(tester);
      expect(work.calls.last, 'stop');
      expect(
        work.calls.where((c) => c.contains('Picking up')),
        isEmpty,
        reason: 'no resume after a cancel',
      );
      expect(work.calls.where((c) => c.startsWith('failed')), isEmpty);
      expect(api.callNames, isNot(contains('createArtifact')));
    });

    testWidgets('a change shows its narration in the notification', (
      tester,
    ) async {
      turns = _Turns();
      final aid = api.seedArtifact('Runs', {'index.html': _page});
      await pumpApp(tester);
      await pumpUntil(tester, () => api.waitingPolls == 1);
      final gate = Completer<void>();
      turns!.steps
        ..add((r) async {
          r.onProgress?.call(const Narration('Renaming the heading'));
          await gate.future;
          return _calls([
            (
              'str_replace',
              {
                'path': 'index.html',
                'old_str': '<h1>Runs</h1>',
                'new_str': '<h1>My runs</h1>',
              },
            ),
            ('render', {'file': null}),
          ]);
        })
        ..add(
          (_) async => _calls([
            ('finish', {'reply': 'Renamed.', 'resolve': true}),
          ]),
        );
      api.sendComment(aid, 'Call it My runs');
      await pumpUntil(
        tester,
        () => work.calls.any((c) => c.endsWith('Renaming the heading)')),
      );
      gate.complete();
      await pumpUntil(tester, () => work.calls.contains('stop'));
      expect(api.currentVersion(aid), 2);
      expect(await api.listDrafts(), isEmpty);
    });

    testWidgets("a change that reads another app's documents asks the owner "
        'in a dialog first, its notification saying Clax needs your OK; '
        'Always is kept for the pair', (tester) async {
      turns = _Turns();
      final week = api.seedArtifact('Week', {'index.html': _page});
      final aid = api.seedArtifact('Runs', {'index.html': _page});
      await pumpApp(tester);
      await pumpUntil(tester, () => api.waitingPolls == 1);
      String? result;
      turns!.steps
        ..add(
          (_) async => _calls([
            ('db_list', {'collection': 'runs', 'app': week, 'limit': null}),
          ]),
        )
        ..add((r) async {
          result = jsonEncode(r.messages.last);
          return _calls([
            ('finish', {'reply': 'Here is your week.', 'resolve': true}),
          ]);
        });
      api.sendComment(aid, 'Show my week here');
      final question = find.text('Let Clax read Week?');
      await pumpUntil(tester, () => question.evaluate().isNotEmpty);
      expect(turns!.turns, 1, reason: 'the run waits for the answer');
      expect(
        work.calls,
        contains('update(Clax needs your OK, Let Clax read Week?)'),
      );
      await tester.tap(find.text('Always for this app'));
      await tester.pump();
      await tester.tap(find.text('Always allow'));
      await pumpUntil(tester, () => work.calls.contains('stop'));
      expect(question, findsNothing);
      expect(result, isNot(contains('did not allow')));
      await settleFiles(tester);
      expect(
        jsonDecode(
          File('${dir.path}/agent-read-consent.json').readAsStringSync(),
        ),
        {
          'always': [
            [aid, week],
          ],
        },
      );
    });

    testWidgets('the app screen\'s Retry goes to the worker', (tester) async {
      final aid = api.seedArtifact('Runs', {'index.html': _page});
      work.launch = aid;
      await pumpApp(tester);
      await pumpRoute(tester);
      final screen = tester.widget<AppScreen>(find.byType(AppScreen));
      expect(screen.onRetry, isNotNull);
      expect(await screen.onRetry!(aid, 'no-such-thread'), isFalse);
    });
  });

  group('on the worker\'s queue', () {
    late List<QueueWiring> wirings;
    late _Run run;

    QueueStarter starter() {
      wirings = [];
      run = _Run();
      return (w) async {
        wirings.add(w);
        return run;
      };
    }

    Future<QueueWiring> started(WidgetTester tester) async {
      await pumpUntil(tester, () => wirings.isNotEmpty);
      return wirings.single;
    }

    testWidgets('starts once the core is ready, with the settings and the '
        'key, rendering in the app\'s inspector', (tester) async {
      await pumpApp(tester, startQueue: starter());
      final w = await started(tester);
      expect(w.settings, AppSettings().toJson());
      expect(w.apiKey, 'sk-ant-x');
      expect(w.render, isA<RenderService>());
      expect((w.render as RenderService).canRender.value, isTrue);
      coreHost.set(const CoreReady());
      await tester.pump();
      expect(wirings, hasLength(1), reason: 'started once');
    });

    testWidgets('a make the last run kept is left for the import, not shown',
        (tester) async {
      // Seeded synchronously: real file futures do not settle in a widget
      // test's body.
      File('${dir.path}/pending-make.json')
          .writeAsStringSync('{"request": "laps"}');
      await pumpApp(tester, startQueue: starter());
      await started(tester);
      await settleFiles(tester);
      expect(find.text('Interrupted'), findsNothing);
      expect(
        File('${dir.path}/pending-make.json').readAsStringSync(),
        '{"request": "laps"}',
      );
    });

    testWidgets('its KeepAlive holds a background run, and the lifecycle '
        'reaches the worker', (tester) async {
      work.pausesRuns = true;
      await pumpApp(tester, startQueue: starter());
      final w = await started(tester);
      work.calls.clear();
      w.onEffect(const q.KeepAlive(1, true));
      await tester.pump();
      expect(work.calls.where((c) => c.startsWith('start(Making your apps')), hasLength(1));
      w.onEffect(const q.KeepAlive(2, false));
      await tester.pump();
      expect(work.calls.last, 'stop');

      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      await tester.pump();
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pump();
      expect(
        run.told.whereType<q.Woke>().map((m) => m.foreground),
        [true],
      );
      work.stopNow();
      await tester.pump();
      expect(run.told.last, isA<q.OutOfTime>());
    });

    testWidgets('on iOS pauseDone waits for the worker\'s Paused, which comes '
        'once its pauses are answered', (tester) async {
      work.pausesRuns = true;
      await pumpApp(tester, startQueue: starter());
      final w = await started(tester);
      w.onEffect(const q.KeepAlive(1, true));
      await tester.pump();
      work.calls.clear();
      work.stopNow();
      await tester.pump();
      expect(run.told.last, isA<q.OutOfTime>());
      // The worker's pause moves are not answered yet.
      await tester.pump(const Duration(milliseconds: 1400));
      expect(work.calls, isNot(contains('pauseDone')));
      w.onEffect(const q.Paused(2));
      await tester.pump();
      expect(work.calls.where((c) => c == 'pauseDone'), hasLength(1));
      await tester.pump(const Duration(seconds: 2));
      expect(work.calls.where((c) => c == 'pauseDone'), hasLength(1));
    });

    testWidgets('on iOS pauseDone goes at the bound when the worker\'s Paused '
        'does not come', (tester) async {
      work.pausesRuns = true;
      await pumpApp(tester, startQueue: starter());
      final w = await started(tester);
      w.onEffect(const q.KeepAlive(1, true));
      await tester.pump();
      work.calls.clear();
      work.stopNow();
      await tester.pump(const Duration(milliseconds: 1400));
      expect(work.calls, isNot(contains('pauseDone')));
      await tester.pump(const Duration(milliseconds: 100));
      expect(work.calls.where((c) => c == 'pauseDone'), hasLength(1));
      // A Paused after the bound says nothing more.
      w.onEffect(const q.Paused(2));
      await tester.pump();
      expect(work.calls.where((c) => c == 'pauseDone'), hasLength(1));
    });

    testWidgets('on iOS with no job running pauseDone goes at once',
        (tester) async {
      work.pausesRuns = true;
      await pumpApp(tester, startQueue: starter());
      await started(tester);
      work.calls.clear();
      work.stopNow();
      await tester.pump();
      expect(work.calls.where((c) => c == 'pauseDone'), hasLength(1));
    });

    testWidgets('a job that ends while the app is away is a notification',
        (tester) async {
      // The made make's plan is looked up for its build: it has none.
      (coreHost.calls as FakeCore).on(
        'GET',
        '/api/jobs/{id}',
        (_, _) async => FakeCore.error(404, 'not_found', 'no such job'),
      );
      await pumpApp(tester, startQueue: starter());
      final w = await started(tester);
      // The worker sends a job's notice before the state that shows it
      // ended: the notice names the app.
      const running = JobView(
        id: 'j1',
        kind: JobKind.make,
        state: JobState.running,
        title: 'Todo',
      );
      const done = JobView(
        id: 'j1',
        kind: JobKind.make,
        state: JobState.done,
        artifactId: 'a1',
        title: 'Todo',
      );
      run.queue.push(const [running]);
      await tester.pump();
      w.onEffect(
        const q.Notice(3, q.NoticeKind.made, 'j1', 'Todo', artifactId: 'a1'),
      );
      run.queue.push(const [done]);
      await tester.pump();
      expect(work.calls.where((c) => c.startsWith('ready(')), isEmpty,
          reason: 'in front the screens say it');
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      await tester.pump();
      run.queue.push(const [running]);
      await tester.pump();
      w.onEffect(
        const q.Notice(4, q.NoticeKind.made, 'j1', 'Todo', artifactId: 'a1'),
      );
      await tester.pump();
      expect(work.calls, contains('ready(Todo is ready, Tap to open it)'));
      expect(work.targets.last, 'a1');
      run.queue.push(const [done]);
      await tester.pump();
    });

    testWidgets('a failed core ends the worker', (tester) async {
      await pumpApp(tester, startQueue: starter());
      await started(tester);
      expect(run.disposed, isFalse);
      coreHost.set(const CoreFailed('core_failed', 'it broke', null));
      await tester.pump();
      expect(run.disposed, isTrue);
    });
  });
}

/// A queue run the app is handed: its client, what the app told it, and
/// whether it was disposed.
final class _Run implements QueueRun {
  final queue = FakeQueueClient();
  final told = <q.Msg>[];
  var disposed = false;

  @override
  QueueClient get client => queue;

  @override
  void tell(q.Msg news) => told.add(news);

  @override
  Future<void> dispose() async => disposed = true;
}
