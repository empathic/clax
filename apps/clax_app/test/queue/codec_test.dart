import 'dart:convert';
import 'dart:isolate';

import 'package:clax_app/queue/codec.dart';
import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

/// The isolate codec (spec 2026-10-19-in-process-core-design §7.1–§7.2):
/// every message, effect and boundary value round-trips exactly, through a
/// `SendPort` and through JSON text, and no input makes a decoder throw.
void main() {
  group('every value round-trips', () {
    for (final (name, m) in msgSamples) {
      test('msg $name', () => roundTrips(m, encodeMsg, decodeMsg, msgFields));
    }
    for (final (name, e) in effectSamples) {
      test('effect $name', () => roundTrips(e, encodeEffect, decodeEffect));
    }
    for (final (name, s) in stateSamples) {
      test(
        'queue state $name',
        () => roundTrips(s, encodeQueueState, decodeQueueState),
      );
    }
    for (final (name, r) in renderRequestSamples) {
      test(
        'render request $name',
        () => roundTrips(r, encodeRenderRequest, decodeRenderRequest),
      );
    }
    for (final (name, r) in renderResultSamples) {
      test(
        'render result $name',
        () => roundTrips(r, encodeRenderResult, decodeRenderResult),
      );
    }
    for (final (name, m) in toUiSamples) {
      test('to the UI $name', () => roundTrips(m, encodeToUi, decodeToUi));
    }
    for (final (name, m) in toWorkerSamples) {
      test(
        'to the worker $name',
        () => roundTrips(
          m,
          encodeToWorker,
          decodeToWorker,
          (w) => switch (w) {
            WorkerMsg(:final msg) => ['msg', ...msgFields(msg)],
            WorkerRender(:final result) => ['render', result],
          },
        ),
      );
    }
  });

  group('the samples cover every tag', () {
    test('every message tag has a sample', () {
      expect({
        for (final (_, m) in msgSamples) encodeMsg(m)['t'],
      }, containsAll(msgTags));
    });
    test('every effect tag has a sample', () {
      expect({
        for (final (_, e) in effectSamples) encodeEffect(e)['t'],
      }, containsAll(effectTags));
    });
    test('message and effect tags are disjoint', () {
      expect(msgTags.toSet().intersection(effectTags.toSet()), isEmpty);
    });
    test('a message does not decode as an effect, nor one as the other', () {
      for (final (_, m) in msgSamples) {
        expect(
          failureOf(decodeEffect(encodeMsg(m))).kind,
          CodecErrorKind.unknownTag,
        );
      }
      for (final (_, e) in effectSamples) {
        expect(
          failureOf(decodeMsg(encodeEffect(e))).kind,
          CodecErrorKind.unknownTag,
        );
      }
    });
  });

  group('order is kept', () {
    test('a job\'s steps keep the order they were committed in', () {
      final wire = encodeMsg(JobsLoaded([fullJob]));
      final back = (okOf(decodeMsg(wire)) as JobsLoaded).jobs.single;
      expect(back.steps.keys.toList(), fullJob.steps.keys.toList());
      expect(back.threads.keys.toList(), fullJob.threads.keys.toList());
      expect(back.questions.keys.toList(), fullJob.questions.keys.toList());
    });
    test('the queue state keeps its jobs\' order', () {
      final s = stateSamples.last.$2;
      final back = okOf(decodeQueueState(encodeQueueState(s)));
      expect([for (final j in back.jobs) j.id], [for (final j in s.jobs) j.id]);
    });
    test('a time keeps its zone', () {
      final local = DateTime(2026, 10, 20, 9, 30, 1, 2, 3);
      final back = okOf(decodeMsg(encodeMsg(Tick(local)))) as Tick;
      expect(back.now.isUtc, isFalse);
      expect(back.now, local);
    });
  });

  group('decoding refuses what it does not know, with a typed error', () {
    final make = encodeMsg(const Make('j', 'r'));
    final cases = <(String, Object?, CodecErrorKind, String)>[
      ('null', null, CodecErrorKind.version, r'$'),
      ('a string', 'make', CodecErrorKind.version, r'$'),
      ('a list', [1, 2], CodecErrorKind.version, r'$'),
      ('no version', {'t': 'make'}, CodecErrorKind.version, r'$.v'),
      (
        'another version',
        {...make, 'v': codecVersion + 1},
        CodecErrorKind.version,
        r'$.v',
      ),
      (
        'a version as text',
        {...make, 'v': '$codecVersion'},
        CodecErrorKind.version,
        r'$.v',
      ),
      ('no tag', {'v': codecVersion}, CodecErrorKind.malformed, r'$.t'),
      (
        'an unknown tag',
        {...make, 't': 'teleport'},
        CodecErrorKind.unknownTag,
        r'$.t',
      ),
      (
        'a field missing',
        {...make}..remove('request'),
        CodecErrorKind.malformed,
        r'$.request',
      ),
      (
        'a field of the wrong type',
        {...make, 'id': 7},
        CodecErrorKind.malformed,
        r'$.id',
      ),
      (
        'a key not a string',
        <Object?, Object?>{...make, 3: 'x'},
        CodecErrorKind.malformed,
        r'$',
      ),
      (
        'an unknown state',
        encodeMsg(const JobNoticed('j', state: JobState.running, rev: 1))
            .withField('state', 'flying'),
        CodecErrorKind.malformed,
        r'$.state',
      ),
      (
        'an unknown nested tag',
        {
          'v': codecVersion,
          't': 'turnDone',
          'effect': 1,
          'jobId': 'j',
          'outcome': {
            'notes': {
              'calls': [
                {'t': 'zap', 'id': 'c'},
              ],
              'data': null,
            },
            'stop': null,
          },
        },
        CodecErrorKind.unknownTag,
        r'$.outcome.notes.calls[0].t',
      ),
      (
        'data holding a non-JSON leaf',
        {
          'v': codecVersion,
          't': 'rendered',
          'effect': 1,
          'jobId': 'j',
          'outcome': {
            'clean': true,
            'data': {'png': Object()},
          },
        },
        CodecErrorKind.malformed,
        r'$.outcome.data.png',
      ),
      (
        'data holding a map with a number key',
        {
          'v': codecVersion,
          't': 'rendered',
          'effect': 1,
          'jobId': 'j',
          'outcome': {
            'clean': true,
            'data': {
              'errors': [
                {1: 'x'},
              ],
            },
          },
        },
        CodecErrorKind.malformed,
        r'$.outcome.data.errors[0]',
      ),
      (
        'data holding a number JSON cannot carry',
        {
          'v': codecVersion,
          't': 'rendered',
          'effect': 1,
          'jobId': 'j',
          'outcome': {'clean': true, 'data': double.nan},
        },
        CodecErrorKind.malformed,
        r'$.outcome.data',
      ),
      (
        'a time out of range',
        {
          'v': codecVersion,
          't': 'tick',
          'now': {'us': 9000000000000000000, 'utc': true},
        },
        CodecErrorKind.malformed,
        r'$.now',
      ),
    ];
    for (final (name, wire, kind, path) in cases) {
      test(name, () {
        final e = failureOf(decodeMsg(wire));
        expect(e.kind, kind, reason: '$e');
        expect(e.path, path, reason: '$e');
      });
    }

    test('an offer named twice', () {
      final view = encodeQueueState(QueueState([fullView]))['jobs']!.asList;
      final e = failureOf(
        decodeQueueState({
          'v': codecVersion,
          'jobs': [
            {
              ...view.first.asMap,
              'offers': ['retry', 'retry'],
            },
          ],
        }),
      );
      expect(e.kind, CodecErrorKind.malformed, reason: '$e');
      expect(e.path, r'$.jobs[0].offers[1]', reason: '$e');
    });

    test('a field the wire does not name is ignored', () {
      final d = decodeMsg({...make, 'later': 1});
      expect(okOf(d), const Make('j', 'r'));
    });

    test('a decoder never throws, whatever field is broken', () {
      const breakers = <Object?>[
        null,
        'x',
        -1,
        9000000000000000000,
        2.5,
        true,
        <Object?>[],
        <String, Object?>{},
        [null],
        {'t': 'turn'},
      ];
      var tried = 0;
      void tryAll<T>(
        Map<String, Object?> wire,
        Decoded<T> Function(Object?) dec,
      ) {
        for (final path in pathsOf(wire)) {
          for (final b in breakers) {
            tried++;
            expect(
              () => dec(replaceAt(wire, path, b)),
              returnsNormally,
              reason: '$path = $b',
            );
          }
          tried++;
          expect(() => dec(removeAt(wire, path)), returnsNormally);
        }
      }

      for (final (_, m) in msgSamples) {
        tryAll(encodeMsg(m), decodeMsg);
      }
      for (final (_, e) in effectSamples) {
        tryAll(encodeEffect(e), decodeEffect);
      }
      for (final (_, s) in stateSamples) {
        tryAll(encodeQueueState(s), decodeQueueState);
      }
      for (final (_, m) in toUiSamples) {
        tryAll(encodeToUi(m), decodeToUi);
      }
      for (final (_, m) in toWorkerSamples) {
        tryAll(encodeToWorker(m), decodeToWorker);
      }
      expect(tried, greaterThan(1000));
    });

    test('a deeply nested data value answers a failure or a value', () {
      Object? deep = 'x';
      for (var i = 0; i < 100000; i++) {
        deep = [deep];
      }
      final wire = {
        'v': codecVersion,
        't': 'rendered',
        'effect': 1,
        'jobId': 'j',
        'outcome': {'clean': true, 'data': deep},
      };
      expect(() => decodeMsg(wire), returnsNormally);
    });
  });

  test('every sample crosses a SendPort and decodes the same', () async {
    final wires = <Map<String, Object?>>[
      for (final (_, m) in toUiSamples) encodeToUi(m),
      for (final (_, m) in toWorkerSamples) encodeToWorker(m),
    ];
    final port = ReceivePort();
    for (final w in wires) {
      port.sendPort.send(w);
    }
    final got = await port.take(wires.length).toList();
    port.close();
    for (var i = 0; i < wires.length; i++) {
      expect(got[i], wires[i]);
    }
    var at = 0;
    for (final (_, m) in toUiSamples) {
      expect(encodeToUi(okOf(decodeToUi(got[at++]))), encodeToUi(m));
    }
    for (final (_, m) in toWorkerSamples) {
      expect(
        encodeToWorker(okOf(decodeToWorker(got[at++]))),
        encodeToWorker(m),
      );
    }
  });
}

// ------------------------------------------------------------ checks

/// [value] encodes to plain JSON, decodes to a value of its type that
/// encodes the same, the same again through JSON text, and decodes equal
/// by [fields] (default: by `==`).
void roundTrips<T>(
  T value,
  Map<String, Object?> Function(T) encode,
  Decoded<T> Function(Object?) decode, [
  Object? Function(T)? fields,
]) {
  final wire = encode(value);
  expect(wire['v'], codecVersion);
  expectPlainJson(wire, r'$');
  final back = okOf(decode(wire));
  expect(back.runtimeType, value.runtimeType);
  final f = fields ?? (T v) => v;
  expect(f(back), f(value));
  expect(encode(back), wire);
  final text = jsonEncode(wire);
  final again = okOf(decode(jsonDecode(text)));
  expect(f(again), f(value));
  expect(jsonEncode(encode(again)), text);
}

void expectPlainJson(Object? v, String path) {
  switch (v) {
    case null || String() || bool() || int():
      return;
    case double():
      expect(v.isFinite, isTrue, reason: path);
    case List():
      for (var i = 0; i < v.length; i++) {
        expectPlainJson(v[i], '$path[$i]');
      }
    case Map():
      for (final e in v.entries) {
        expect(e.key, isA<String>(), reason: path);
        expectPlainJson(e.value, '$path.${e.key}');
      }
    default:
      fail('$path holds ${v.runtimeType}');
  }
}

T okOf<T>(Decoded<T> d) => switch (d) {
  DecodeOk(:final value) => value,
  DecodeFailure(:final error) => fail('did not decode: $error'),
};

CodecError failureOf<T>(Decoded<T> d) => switch (d) {
  DecodeOk(:final value) => fail('decoded $value'),
  DecodeFailure(:final error) => error,
};

/// Each message's fields, for the messages that do not compare by value.
List<Object?> msgFields(Msg m) => [
  m.runtimeType,
  ...switch (m) {
    Intent() => [m],
    CoreOpened() || OutOfTime() => const [],
    OwnerClaimed(:final effect, :final token) => [effect, token],
    OwnerHeld(:final effect) || OwnerDraining(:final effect) => [effect],
    OwnerChanged(:final client, :final held) => [client, held],
    JobsLoaded(:final jobs) => jobs,
    JobNoticed() => [m],
    JobRead(:final effect, :final jobId, :final job) => [effect, jobId, job],
    Committed(:final effect, :final job, :final step, :final replayed) => [
      effect,
      job,
      step,
      replayed,
    ],
    Refused(:final effect, :final jobId, :final reason, :final job) => [
      effect,
      jobId,
      reason.runtimeType,
      ...switch (reason) {
        Invalid(:final code, :final message) => [code, message],
        _ => [reason],
      },
      job,
    ],
    TurnDone(:final effect, :final jobId, :final outcome) => [
      effect,
      jobId,
      outcome,
    ],
    TurnFailed(:final effect, :final jobId, :final failure, :final detail) => [
      effect,
      jobId,
      failure,
      detail,
    ],
    ToolDone(:final effect, :final jobId, :final callId, :final result) => [
      effect,
      jobId,
      callId,
      result,
    ],
    Rendered(:final effect, :final jobId, :final outcome) => [
      effect,
      jobId,
      outcome,
    ],
    RenderLost(:final effect, :final jobId) => [effect, jobId],
    CanRender(:final can) => [can],
    ClaudeReachable(:final reachable) => [reachable],
    Tick(:final now) => [now, now.isUtc],
    Woke(:final foreground) => [foreground],
  },
];

// ------------------------------------------------------------ wire surgery

extension on Map<String, Object?> {
  Map<String, Object?> withField(String k, Object? v) => {...this, k: v};
}

extension on Object? {
  List<Object?> get asList => this! as List<Object?>;
  Map<String, Object?> get asMap => this! as Map<String, Object?>;
}

/// Every path into [v]: map keys and list indices.
List<List<Object>> pathsOf(Object? v, [List<Object> at = const []]) => [
  if (at.isNotEmpty) at,
  if (v is Map<String, Object?>)
    for (final e in v.entries) ...pathsOf(e.value, [...at, e.key]),
  if (v is List<Object?>)
    for (var i = 0; i < v.length; i++) ...pathsOf(v[i], [...at, i]),
];

Object? replaceAt(Object? v, List<Object> path, Object? to) {
  if (path.isEmpty) return to;
  final [head, ...rest] = path;
  return switch (v) {
    Map<String, Object?>() => {
      ...v,
      head as String: replaceAt(v[head], rest, to),
    },
    List<Object?>() => [
      for (var i = 0; i < v.length; i++)
        i == head ? replaceAt(v[i], rest, to) : v[i],
    ],
    _ => v,
  };
}

Object? removeAt(Object? v, List<Object> path) {
  final [head, ...rest] = path;
  return switch (v) {
    Map<String, Object?>() when rest.isEmpty => {...v}..remove(head),
    Map<String, Object?>() => {...v, head as String: removeAt(v[head], rest)},
    List<Object?>() when rest.isEmpty => [...v]..removeAt(head as int),
    List<Object?>() => [
      for (var i = 0; i < v.length; i++)
        i == head ? removeAt(v[i], rest) : v[i],
    ],
    _ => v,
  };
}

// ------------------------------------------------------------ samples

final t0 = DateTime.utc(2026, 10, 20, 9, 30, 1, 2, 3);

const notes = TurnNotes(
  calls: [
    FinishCall(
      'c1',
      PublishPlan(
        base: 3,
        data: {
          'files': ['index.html'],
          'size': 12.5,
        },
      ),
      reply: ReplyPlan(data: 'Done.'),
    ),
    FinishCall('c1b', PublishPlan(base: 0)),
    RenderCall(
      'c2',
      RenderSpec(draftId: 'd1', revision: 2, kind: RenderKind.tests),
    ),
    AskCall(
      'c3',
      Question(
        data: {
          'questions': [
            {'q': 'Blue?', 'options': <Object?>[]},
          ],
        },
      ),
    ),
    OtherCall('c4', 'read', args: {'path': 'a.html', 'lines': null}),
    OtherCall('c5', 'list'),
  ],
  data: {'text': 'I made it.', 'tokens': 1200},
);

final fullJob = Job(
  id: 'j1',
  kind: JobKind.change,
  state: JobState.waiting,
  step: const PublishKey(3),
  rev: 7,
  artifactId: 'a1',
  title: 'Tidy the list',
  input: const JobInput(
    request: 'Tidy it',
    base: 3,
    threads: {'t1': 'c1', 't2': 'c9'},
  ),
  plan: const PlanState(
    planRest: true,
    data: {
      'steps': [1, 2.5, null, true, 's'],
    },
  ),
  carry: const Carry(stops: 2, canKeepGoing: true, data: {'draft': 'd1'}),
  error: const JobError('version_moved', 5, PublishKey(3)),
  steps: {
    const TurnKey(2): const TurnStepResult(notes, base: 3),
    const TurnKey(1): const TurnStepResult(TurnNotes()),
    const PublishKey(3): const PublishStepResult(artifactId: 'a1', version: 4),
    const ReplyKey(): const ReplyStepResult(data: {'ok': true}),
    const UnknownKey('turn:0'): const ReplyStepResult(),
  },
  threads: {'t2': ('c9', true), 't1': ('c1', false)},
  questions: {
    'q2': const JobQuestion(Answered({'a': 1}), toolUseId: 'j1/turn:1/c3'),
    'q1': const JobQuestion(AnswerPending()),
    'q3': const JobQuestion(Declined()),
    'q4': const JobQuestion(Withdrawn(), toolUseId: 'x'),
  },
);

const bareJob = Job(
  id: 'j2',
  kind: JobKind.make,
  state: JobState.queued,
  rev: 1,
  input: JobInput(request: ''),
);

const fullView = JobView(
  id: 'j1',
  kind: JobKind.change,
  state: JobState.failed,
  waitingFor: WaitingFor.render,
  step: TurnKey(4),
  title: 'Tidy',
  artifactId: 'a1',
  error: 'doc_moved',
  offers: {Offer.retry, Offer.discard},
);

final turnInput = TurnInput(
  turn: 3,
  input: fullJob.input,
  base: 4,
  previous: notes,
  results: {
    'c4': const ToolResult(data: 'contents'),
    'c2': ToolResult.refused('version_moved', 5),
    'c3': ToolResult.answer(const Declined()),
  },
  carry: fullJob.carry,
  planRest: true,
);

const outcome = RenderOutcome(
  clean: false,
  data: {
    'errors': ['x is not defined'],
    'png': 'iVBOR',
  },
);

final msgSamples = <(String, Msg)>[
  ('make', const Make('j1', 'A list', title: 'List')),
  ('make, untitled', const Make('j1', '')),
  (
    'change',
    const Change(
      'j2',
      'a1',
      base: 3,
      threads: {'t1': 'c1', 't2': 'c2'},
      request: 'Tidy',
      title: 'T',
    ),
  ),
  ('change, bare', const Change('j2', 'a1', base: 0, threads: {})),
  ('cancel', const Cancel('j1')),
  ('retry', const Retry('j1')),
  ('keep going', const KeepGoing('j1')),
  ('discard', const Discard('j1')),
  ('core opened', const CoreOpened()),
  ('owner claimed', const OwnerClaimed(4, 'tok')),
  ('owner held', const OwnerHeld(4)),
  ('owner draining', const OwnerDraining(4)),
  ('owner changed', const OwnerChanged(client: 'cli', held: true)),
  ('owner released', const OwnerChanged(held: false)),
  ('jobs loaded', JobsLoaded([fullJob, bareJob])),
  ('no jobs loaded', const JobsLoaded([])),
  (
    'job noticed',
    const JobNoticed('j1', state: JobState.running, step: ReplyKey(), rev: 9),
  ),
  (
    'job noticed, no step',
    const JobNoticed('j1', state: JobState.done, rev: 1),
  ),
  ('job read', JobRead(5, 'j1', fullJob)),
  ('job read, none', const JobRead(5, 'j1', null)),
  (
    'committed',
    Committed(
      6,
      fullJob,
      step: const PublishStepResult(artifactId: 'a1', version: 4),
      replayed: true,
    ),
  ),
  ('committed, no step', const Committed(6, bareJob)),
  (
    'refused, version moved',
    Refused(7, 'j1', const VersionMoved(8), job: fullJob),
  ),
  ('refused, doc moved', const Refused(7, 'j1', DocMoved('notes/a.md'))),
  ('refused, not rendered clean', const Refused(7, 'j1', NotRenderedClean())),
  ('refused, job moved', const Refused(7, 'j1', JobMoved(), job: bareJob)),
  ('refused, not owner', const Refused(7, 'j1', NotOwner())),
  ('refused, invalid', const Refused(7, 'j1', Invalid('invalid_move', 'no'))),
  ('refused, invalid bare', const Refused(7, 'j1', Invalid('job_exists'))),
  (
    'turn done',
    const TurnDone(
      8,
      'j1',
      TurnOutcome(notes, stop: TurnStop(canKeepGoing: true, data: [1])),
    ),
  ),
  ('turn done, no stop', const TurnDone(8, 'j1', TurnOutcome(TurnNotes()))),
  (
    'turn failed',
    const TurnFailed(9, 'j1', TurnFailure.network, detail: 'offline'),
  ),
  ('turn failed, bare', const TurnFailed(9, 'j1', TurnFailure.other)),
  (
    'tool done',
    const ToolDone(10, 'j1', 'c4', ToolResult(data: {'a': 1}, isError: true)),
  ),
  ('tool done, bare', const ToolDone(10, 'j1', 'c4', ToolResult())),
  ('rendered', const Rendered(11, 'j1', outcome)),
  ('render lost', const RenderLost(11, 'j1')),
  ('can render', const CanRender(false)),
  ('claude reachable', const ClaudeReachable(true)),
  ('tick', Tick(t0)),
  ('woke', const Woke(foreground: true)),
  ('out of time', const OutOfTime()),
];

final effectSamples = <(String, Effect)>[
  (
    'create job',
    CreateJob(
      1,
      'j1',
      kind: JobKind.change,
      input: fullJob.input,
      artifactId: 'a1',
      title: 'T',
      plan: fullJob.plan,
    ),
  ),
  (
    'create job, bare',
    const CreateJob(
      1,
      'j1',
      kind: JobKind.make,
      input: JobInput(request: 'r'),
    ),
  ),
  (
    'move job, set',
    MoveJob(
      2,
      'j1',
      ifRev: 3,
      to: JobState.stopped,
      error: SetTo(fullJob.error!),
      carry: SetTo(fullJob.carry!),
      plan: SetTo(fullJob.plan!),
    ),
  ),
  (
    'move job, clear',
    const MoveJob(
      2,
      'j1',
      ifRev: 3,
      to: JobState.running,
      error: Clear(),
      carry: Clear(),
      plan: Clear(),
    ),
  ),
  ('move job, keep', const MoveJob(2, 'j1', ifRev: 3, to: JobState.cancelled)),
  ('commit step', const CommitStep(3, 'j1', 2, notes, base: 3)),
  ('ask', const Ask(4, 'j1', 2, 'c3', Question(data: {'q': 'Blue?'}))),
  (
    'publish',
    const Publish(
      5,
      'j1',
      2,
      'c1',
      PublishPlan(base: 3, data: {'files': <Object?>[]}),
      done: true,
    ),
  ),
  ('reply', const Reply(6, 'j1', 2, 'c1', ReplyPlan(data: {'text': 'Done.'}))),
  ('start turn', StartTurn(7, 'j1', turnInput)),
  (
    'start turn, bare',
    const StartTurn(
      7,
      'j1',
      TurnInput(turn: 1, input: JobInput(request: 'r'), base: 0),
    ),
  ),
  (
    'run tool',
    const RunTool(8, 'j1', 2, OtherCall('c4', 'read', args: {'p': 'a'})),
  ),
  (
    'render',
    Render(
      9,
      'j1',
      2,
      'c2',
      const RenderSpec(draftId: 'd1', revision: 2, kind: RenderKind.migration),
      last: true,
      deadline: t0,
    ),
  ),
  (
    'render, bare',
    const Render(9, 'j1', 2, 'c2', RenderSpec(draftId: 'd1', revision: 0)),
  ),
  ('read job', const ReadJob(10, 'j1')),
  ('stop work', const StopWork(11, 'j1')),
  ('claim owner', const ClaimOwner(12)),
  ('load jobs', const LoadJobs(13)),
  ('probe claude', const ProbeClaude(14)),
  ('notice', const Notice(15, NoticeKind.stopped, 'j1', 'Tidy')),
  ('keep alive', const KeepAlive(16, true)),
  ('publish state', PublishState(17, stateSamples.last.$2)),
];

final stateSamples = <(String, QueueState)>[
  ('empty', const QueueState([])),
  (
    'jobs',
    const QueueState([
      fullView,
      JobView(id: 'j2', kind: JobKind.make, state: JobState.queued),
      JobView(
        id: 'j0',
        kind: JobKind.make,
        state: JobState.stopped,
        step: UnknownKey('odd'),
        offers: {Offer.keepGoing, Offer.discard},
      ),
    ]),
  ),
];

final renderRequestSamples = <(String, RenderRequest)>[
  (
    'inspect',
    const RenderRequest(9, 'j1', RenderSpec(draftId: 'd1', revision: 2)),
  ),
];

final renderResultSamples = <(String, RenderResult)>[
  ('answered', const RenderAnswered(9, 'j1', outcome)),
  ('answered clean', const RenderAnswered(9, 'j1', RenderOutcome(clean: true))),
  ('gone', const RenderGone(9, 'j1')),
];

final toUiSamples = <(String, ToUi)>[
  for (final (n, e) in effectSamples) ('effect $n', UiEffect(e)),
  for (final (n, r) in renderRequestSamples) ('render $n', UiRender(r)),
];

final toWorkerSamples = <(String, ToWorker)>[
  for (final (n, m) in msgSamples) ('msg $n', WorkerMsg(m)),
  for (final (n, r) in renderResultSamples) ('render $n', WorkerRender(r)),
];
