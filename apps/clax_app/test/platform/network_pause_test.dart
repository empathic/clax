import 'dart:async';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/request.dart';
import 'package:clax_app/platform/network_pause.dart';
import 'package:flutter/widgets.dart' show AppLifecycleState;
import 'package:flutter_test/flutter_test.dart';

const _result = EditResult(
  reply: 'ok',
  title: 'T',
  files: [],
  edits: [],
  removed: [],
  db: [],
  capabilities: {},
  resolve: false,
);

const _dns = AgentError(
  "Couldn't reach the Claude API: SocketException: Failed host lookup: "
  "'api.anthropic.com' (OS Error: No address associated with hostname, "
  'errno = 7)',
  retryable: true,
);

/// Answers each run with the next item: an [AgentError] is thrown, a
/// callback runs first (to change the app's presence mid-run).
class _Backend implements AgentBackend {
  _Backend(this.script);
  final List<Object> script;
  int runs = 0;

  @override
  String get name => 'fake';

  @override
  Future<EditResult> run(AgentRequest request) async {
    final item = script[runs++];
    if (item is void Function()) {
      item();
      throw _dns;
    }
    if (item is AgentError) throw item;
    return item as EditResult;
  }
}

AgentRequest _request([Future<void>? cancelled]) => AgentRequest(
  kind: RequestKind.newApp,
  model: 'm',
  systemPrompt: 's',
  userText: 'u',
  cancelled: cancelled,
);

void main() {
  test('presence counts leaves and wakes waiters on return', () async {
    final p = AppPresence();
    expect(p.foreground, isTrue);
    p.onLifecycle(AppLifecycleState.inactive);
    expect((p.foreground, p.leaves), (true, 0));
    p.onLifecycle(AppLifecycleState.hidden);
    p.onLifecycle(AppLifecycleState.paused);
    expect((p.foreground, p.leaves), (false, 1));
    var back = false;
    unawaited(p.whenForeground.then((_) => back = true));
    await pumpEventQueue();
    expect(back, isFalse);
    p.onLifecycle(AppLifecycleState.resumed);
    await pumpEventQueue();
    expect(back, isTrue);
  });

  test('network failures are told from the others', () {
    expect(isNetworkCut(_dns), isTrue);
    expect(
      isNetworkCut(
        const AgentError(
          'The connection to the Claude API was lost: Connection reset',
        ),
      ),
      isTrue,
    );
    expect(isNetworkCut(const AgentError('The API key was refused')), isFalse);
    expect(isNetworkCut(const AgentError('The Claude API timed out')), isFalse);
    expect(isNetworkCut(const AgentError.cancelled()), isFalse);
  });

  test('a cut while backgrounded pauses until the app is back, then runs '
      'again', () async {
    final p = AppPresence();
    final events = <String>[];
    final inner = _Backend([
      () => p.onLifecycle(AppLifecycleState.paused),
      _result,
    ]);
    final b = ResumingBackend(
      inner,
      p,
      onPause: () => events.add('pause'),
      onResume: () => events.add('resume'),
    );
    EditResult? got;
    unawaited(b.run(_request()).then((r) => got = r));
    await pumpEventQueue();
    expect(events, ['pause']);
    expect((inner.runs, got), (1, null));
    p.onLifecycle(AppLifecycleState.resumed);
    await pumpEventQueue();
    expect(events, ['pause', 'resume']);
    expect(inner.runs, 2);
    expect(got, same(_result));
  });

  test('a cut seen after the app came back still runs again at once', () async {
    final p = AppPresence();
    final inner = _Backend([
      () {
        p.onLifecycle(AppLifecycleState.paused);
        p.onLifecycle(AppLifecycleState.resumed);
      },
      _result,
    ]);
    expect(await ResumingBackend(inner, p).run(_request()), same(_result));
    expect(inner.runs, 2);
  });

  test('a cut while the app stayed in front fails as before', () async {
    final inner = _Backend([_dns]);
    await expectLater(
      ResumingBackend(inner, AppPresence()).run(_request()),
      throwsA(same(_dns)),
    );
    expect(inner.runs, 1);
  });

  test('other failures are not resumed', () async {
    final p = AppPresence()..onLifecycle(AppLifecycleState.paused);
    const refused = AgentError('The API key was refused');
    await expectLater(
      ResumingBackend(_Backend([refused]), p).run(_request()),
      throwsA(same(refused)),
    );
  });

  test('resumes stop after maxResumes', () async {
    final p = AppPresence();
    void leaveAndReturn() {
      p.onLifecycle(AppLifecycleState.paused);
      p.onLifecycle(AppLifecycleState.resumed);
    }

    final inner = _Backend([leaveAndReturn, leaveAndReturn, leaveAndReturn]);
    await expectLater(
      ResumingBackend(inner, p, maxResumes: 2).run(_request()),
      throwsA(same(_dns)),
    );
    expect(inner.runs, 3);
  });

  test('a cancel while paused ends the run as cancelled', () async {
    final p = AppPresence();
    final cancel = Completer<void>();
    final inner = _Backend([() => p.onLifecycle(AppLifecycleState.paused)]);
    final run = ResumingBackend(inner, p).run(_request(cancel.future));
    await pumpEventQueue();
    cancel.complete();
    await expectLater(
      run,
      throwsA(
        isA<AgentError>().having(
          (e) => e.kind,
          'kind',
          AgentErrorKind.cancelled,
        ),
      ),
    );
    expect(inner.runs, 1);
  });
}
