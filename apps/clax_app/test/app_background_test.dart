import 'dart:async';
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
import 'package:clax_app/daemon/supervisor.dart';
import 'package:clax_app/platform/background_work.dart';
import 'package:clax_app/platform/pending_make.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'agent/loop_support.dart' show RecordingTimings, change, edit;
import 'agent/tools/support.dart' show StepClock;
import 'support/fake_inspector.dart';
import 'support/fake_supervisor.dart';
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
        r.onProgress?.call(ToolStarted(name, toolLabel(name, input)));
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

/// Records every [BackgroundWork] call as `method(args…)`, and plays the
/// platform's launch artifact and taps.
class _Work implements BackgroundWork {
  final calls = <String>[];
  String? launch;
  final _opened = StreamController<String>.broadcast();

  void tap(String aid) => _opened.add(aid);

  @override
  Stream<void> get stopped => const Stream.empty();

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

  @override
  Future<void> ready(String title, String text, String artifactId) async =>
      calls.add('ready($title, $text)');

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
}

void main() {
  late Directory dir;
  late FakeSupervisor supervisor;
  late FakeClaxApi api;
  late _Agent agent;
  late _Work work;
  late List<FakeAppSurface> surfaces;
  _Turns? turns;

  setUp(() {
    turns = null;
    dir = Directory.systemTemp.createTempSync('app-bg-test-');
    supervisor = FakeSupervisor();
    api = FakeClaxApi();
    agent = _Agent();
    work = _Work();
    surfaces = [];
  });
  tearDown(() => dir.deleteSync(recursive: true));

  Future<void> pumpApp(WidgetTester tester) async {
    await tester.pumpWidget(
      AppRoot(
        services: AppServices(
          supervisor: supervisor,
          api: (_) => api,
          backend: (_, _) => turns ?? agent,
          voice: FakeVoice.unsupported('No speech here'),
          settings: SettingsStore(File('${dir.path}/settings.json')),
          key: _Key(),
          timings: RecordingTimings(),
          agentTimings: RecordingTimings(),
          surface: (_) {
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
          inspector: turns == null ? null : FakeInspector(),
          clock: StepClock(),
        ),
      ),
    );
    await tester.pump();
    supervisor.set(Up(endpoint()));
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

  const title = 'Making “a tracker for my runs”…';

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
      'start($title, Starting the agent)',
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
    expect(find.text("Couldn't make it"), findsNothing);
    await pumpUntil(tester, () => agent.runs == 2);

    leave(tester);
    gate.complete();
    await pumpUntil(
      tester,
      () => work.calls.contains('ready(Runs is ready, Tap to open it)'),
    );
    expect(work.calls, [
      'requestPermission',
      'start($title, Starting the agent)',
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

  testWidgets('a network failure while the app stayed in front still fails, '
      'and in the background a failure is posted', (tester) async {
    await pumpApp(tester);
    agent.steps.add((_) async => throw _dns);
    await submit(tester, 'a tracker for my runs');
    await pumpRoute(tester);
    expect(find.text("Couldn't make it"), findsOneWidget);
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
      "failed(Couldn't make your app, The API key was refused)",
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
    expect(find.text("Couldn't make it"), findsOneWidget);
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
        code: 'daemon_unreachable',
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

  testWidgets('a make the last run never finished is offered again at '
      'launch', (tester) async {
    // Seeded synchronously: real file futures do not settle in a widget
    // test's body.
    File('${dir.path}/pending-make.json')
        .writeAsStringSync('{"request": "a tracker for my runs"}');
    agent.steps.add((_) async => _made);
    await pumpApp(tester);
    await pumpUntilShown(tester, 'Make it again?');
    expect(
      find.text('Clax closed before it finished “a tracker for my runs”.'),
      findsOneWidget,
    );
    await tester.tap(find.text('Make it again'));
    await pumpRoute(tester);
    await pumpRoute(tester);
    expect(agent.runs, 1);
    expect(find.byType(AppScreen), findsOneWidget);
    await settleFiles(tester);
    expect(File('${dir.path}/pending-make.json').existsSync(), isFalse);
  });

  testWidgets('Not now forgets the lost make', (tester) async {
    File('${dir.path}/pending-make.json')
        .writeAsStringSync('{"request": "laps"}');
    await pumpApp(tester);
    await pumpUntilShown(tester, 'Not now');
    await tester.tap(find.text('Not now'));
    await pumpRoute(tester);
    expect(find.text('Make it again?'), findsNothing);
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
        'start($title, Starting the agent)',
        'update($title, Writing the tracker)',
        'update($title, Naming the app)',
        'update($title, Writing index.html)',
        'update($title, Checking how it looks)',
        'update($title, Publishing)',
        'stop',
      ]);
      final created = api.calls.singleWhere((c) => c.name == 'createArtifact');
      expect((created.args['request']! as PublishRequest).title, 'Runs');
      expect(await api.listDrafts(), isEmpty);
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
      turns!.steps.add((r) async {
        leave(tester);
        throw _dns;
      });
      await submit(tester, 'a tracker for my runs');
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
}
