import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/agent/backend.dart' show AgentError;
import 'package:clax_app/agent/runner/turn.dart';
import 'package:clax_app/agent/tools/budget.dart' show Usage;
import 'package:clax_app/agent/unfinished.dart' show fingerprint;
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/json_import.dart';
import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/render.dart';
import 'package:clax_app/queue/runner.dart';
import 'package:clax_app/queue/worker_main.dart' show WorkerContext;
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../queue/make_real_test.dart' show ended, startHost;
import '../queue/queue_host_test.dart' show stateWhere;
import '../queue/worker_agent_test.dart' show unscriptedFile;
import '../support/paths.dart';
import '../support/real_core.dart';

/// The fixtures' artifacts (test/fixtures/json-state), replaced by the
/// real core's.
const _reading = 'k3m9q2v7x1ab';
const _habits = 'p4n8r6t2w5cd';

/// The worker's JSON import against the real embedded core: the queue's
/// [EffectRunner] claims the queue, imports the fixtures before it loads
/// the jobs, and loads the jobs the import made.
void main() {
  test(
    'the jobs the import names load with the queue, and a second import is '
    'already done',
    () async {
      final core = await RealCore.open();
      final api = core.api();
      await api.registerAppSession(
        installId: 'install-1',
        pid: pid,
        cwd: core.dir.path,
      );
      Future<String> app(String title) async => (await api.createArtifact(
        PublishRequest(title: title, files: {'index.html': '<p>$title</p>'}),
      )).artifact.id;
      final reading = await app('Reading list');
      final habits = await app('Habits');

      final support = Directory('${core.dir.path}/state')..createSync();
      void seed() {
        for (final n in jsonStateFiles) {
          final text = File(
            appPath('test/fixtures/json-state/$n'),
          ).readAsStringSync();
          File('${support.path}/$n').writeAsStringSync(
            text.replaceAll(_reading, reading).replaceAll(_habits, habits),
          );
        }
      }

      seed();
      final summaries = <ImportSummary>[];
      final loaded = StreamController<List<Job>>.broadcast();
      final sent = <Msg>[];
      final claim = Completer<ClaimAnswer>();
      final transport = CoreTransport(core.host.calls);
      addTearDown(transport.close);
      final runner = EffectRunner(
        core: transport,
        agent: _Unused(),
        render: _Unused(),
        ui: _Unused(),
        probeClaude: () async => true,
        session: () async => throw UnimplementedError(),
        send: (m) {
          sent.add(m);
          if (m is ClaimAnswer && !claim.isCompleted) claim.complete(m);
          if (m is JobsLoaded) loaded.add(m.jobs);
        },
        importState: (c, token) async {
          final s = await importJsonState(
            supportDir: support.path,
            core: c,
            ownerToken: token,
          );
          if (s != null) summaries.add(s);
          return s;
        },
      )..start();
      addTearDown(runner.close);

      runner.run(const ClaimOwner(1));
      expect(
        await claim.future.timeout(const Duration(minutes: 1)),
        isA<OwnerClaimed>(),
      );

      final first = loaded.stream.first;
      runner.run(const LoadJobs(2));
      final jobs = await first.timeout(const Duration(minutes: 1));

      final s = summaries.single;
      expect(s.already, isFalse);
      expect(s.skipped, isEmpty);
      expect(s.consents, 1);
      expect(
        s.leftOut,
        hasLength(3),
        reason: 'the question, the stopped thread and the unfinished change',
      );
      expect(s.leftOut.where((l) => l.startsWith('asked.json:')), hasLength(1));
      expect(s.leftOut.where((l) => l.startsWith('stopped.json:')), hasLength(1));
      // The unfinished change's thread is not open in the core (it never
      // held it), so the app would forget the change: it is not imported.
      expect(
        s.leftOut.where((l) => l.startsWith('unfinished.json:')),
        hasLength(1),
      );
      expect(jobs, hasLength(s.jobs), reason: 'every job the import made is open');
      expect(jobs.every((j) => j.id.startsWith('json-')), isTrue);
      final makes = jobs.where((j) => j.kind == JobKind.make).toList();
      final changes = jobs.where((j) => j.kind == JobKind.change).toList();
      expect(makes.map((j) => j.input.request).toSet(), {
        'a tracker for my runs',
        'a habit log with streaks',
        'a reading list I can sort by author',
      });
      expect(
        makes.singleWhere((j) => j.artifactId == reading).state,
        JobState.stopped,
        reason: 'a plan with a feature to build',
      );
      expect(
        makes.singleWhere((j) => j.input.request == 'a tracker for my runs').plan,
        isNotNull,
        reason: 'its plan was kept under its request',
      );
      expect(
        {for (final j in changes) j.artifactId: j.state},
        {reading: JobState.queued},
        reason: 'the ledger record to run again',
      );
      for (final n in jsonStateFiles) {
        expect(File('${support.path}/$n').existsSync(), isFalse, reason: n);
        expect(File('${s.copiedTo}/$n').existsSync(), isTrue, reason: n);
      }

      // The files back (a crash before the deletes): the core changes
      // nothing, and they go.
      seed();
      final second = loaded.stream.first;
      runner.run(const LoadJobs(3));
      final again = await second.timeout(const Duration(minutes: 1));
      expect(summaries.last.already, isTrue);
      expect(summaries.last.jobs, s.jobs);
      expect(again.map((j) => j.id).toSet(), jobs.map((j) => j.id).toSet());
      for (final n in jsonStateFiles) {
        expect(File('${support.path}/$n').existsSync(), isFalse, reason: n);
      }
    },
    skip: RealCore.skip,
    timeout: const Timeout(Duration(minutes: 3)),
  );

  test(
    'a ledger change whose version landed and whose reply did not posts '
    'its reply from the worker and ends done, asking the model nothing',
    () async {
      final core = await RealCore.open();
      final api = core.api();
      await api.registerAppSession(
        installId: 'install-0',
        pid: pid,
        cwd: core.dir.path,
      );
      final aid = (await api.createArtifact(
        PublishRequest(title: 'Habits', files: {'index.html': '<p>Habits</p>'}),
      )).artifact.id;
      final tid = await _thread(api, aid, 'a header, please');
      final cid = (await api.threads(aid))
          .singleWhere((t) => t.id == tid)
          .comments
          .single
          .id;
      // The change the app's ledger kept: version 2, published from the
      // app's session, addressing the thread; the app ended before its
      // reply.
      const reply = 'Added a header.';
      await api.publish(
        aid,
        PublishRequest(
          ifVersion: 1,
          note: reply,
          addresses: [tid],
          files: {'index.html': '<h1>Habits</h1>'},
        ),
      );

      final (host, support) = startHost(
        core,
        noTurns,
        importJson: true,
        seed: (dir) => File('${dir.path}/change-ledger.json').writeAsStringSync(
          jsonEncode({
            'records': [
              {
                'artifact_id': aid,
                'base': 1,
                'addresses': [tid],
                'answered': {
                  tid: [cid],
                },
                'reply': reply,
                'resolve': false,
                'db': <Object?>[],
                'pins': <String, Object?>{},
                'docs_done': true,
              },
            ],
          }),
        ),
      );
      final job = await ended(host, support);
      expect(job.state, JobState.done, reason: '${job.error}');
      final thread = (await api.threads(aid)).singleWhere((t) => t.id == tid);
      expect(
        [
          for (final c in thread.comments)
            if (c.byAgent) c.body,
        ],
        [reply],
      );
      expect((await api.getArtifact(aid)).artifact.currentVersion, 2);
    },
    skip: RealCore.skip,
    timeout: const Timeout(Duration(minutes: 3)),
  );

  test(
    'Keep going on an imported unfinished change builds on its carried '
    'draft and publishes it',
    () async {
      final core = await RealCore.open();
      final api = core.api();
      await api.registerAppSession(
        installId: 'install-0',
        pid: pid,
        cwd: core.dir.path,
      );
      final aid = (await api.createArtifact(
        PublishRequest(title: 'Habits', files: {'index.html': _old}),
      )).artifact.id;
      final tid = await _thread(api, aid, 'say it is half done');
      final cid = (await api.threads(aid))
          .singleWhere((t) => t.id == tid)
          .comments
          .single
          .id;

      final (host, support) = startHost(
        core,
        keepGoingModel,
        importJson: true,
        seed: (dir) => File('${dir.path}/unfinished.json').writeAsStringSync(
          jsonEncode({
            'version': 1,
            'threads': {
              tid: {
                'artifact': aid,
                'threads': [tid],
                'comments': [cid],
                'limit': 'turns',
                'spent_usd': 1.0,
                'at': '2026-10-18T10:00:00.000Z',
                'stops': 1,
                'notes': ['Ran out of turns'],
                'edits': {'index.html': _half},
                'before': {'index.html': fingerprint(_old)},
              },
            },
          }),
        ),
      );
      final stopped = (await stateWhere(
        host.client,
        (s) => s.jobs.isNotEmpty && s.jobs.single.state == JobState.stopped,
      ).timeout(const Duration(minutes: 1))).jobs.single;
      host.client.send(KeepGoing(stopped.id));

      final job = await ended(host, support);
      expect(job.state, JobState.done, reason: '${job.error}');
      expect((await api.getArtifact(aid)).artifact.currentVersion, 2);
      expect(
        await api.readFiles(aid, 2),
        containsPair('index.html', _half),
        reason: 'the carried draft, which the run only rendered',
      );
    },
    skip: RealCore.skip,
    timeout: const Timeout(Duration(minutes: 3)),
  );
}

/// The page an unfinished change's draft was made on, and the draft.
const _old = '<title>Habits</title><p>Habits</p>';
const _half = '<title>Habits</title><p>Habits, half done</p>';

/// The model of a Keep going run that builds on its carried draft: it
/// renders the draft as it is, then finishes. Any other turn, or a turn
/// after an error, is written to [unscriptedFile] and fails.
TurnBackend keepGoingModel(WorkerContext context) =>
    _KeepGoingModel(context.start.supportDir);

final class _KeepGoingModel implements TurnBackend {
  _KeepGoingModel(this.dir);
  final String dir;

  @override
  String get name => 'scripted';

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    final asked = [
      for (final m in r.messages)
        if (m['role'] == 'assistant') m,
    ].length;
    final last = jsonEncode(r.messages.last['content']);
    if (asked > 1 || last.contains('"is_error":true')) {
      File('$dir/$unscriptedFile').writeAsStringSync(
        'turn after $asked asked: ${jsonEncode(r.messages)}',
      );
      throw AgentError('turn after $asked was not scripted');
    }
    Map<String, Object?> use(String id, String name, Map<String, Object?> i) =>
        {'type': 'tool_use', 'id': id, 'name': name, 'input': i};
    return ModelTurn(
      content: asked == 0
          ? [use('r', 'render', {})]
          : [use('f', 'finish', {'reply': 'Finished it.'})],
      stopReason: 'tool_use',
      usage: const Usage(input: 100, output: 50),
    );
  }
}

/// The model of a worker that must ask it nothing: a turn is written to
/// [unscriptedFile] and fails.
TurnBackend noTurns(WorkerContext context) =>
    _NoTurns(context.start.supportDir);

final class _NoTurns implements TurnBackend {
  _NoTurns(this.dir);
  final String dir;

  @override
  String get name => 'none';

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    File('$dir/$unscriptedFile').writeAsStringSync(jsonEncode(r.messages));
    throw AgentError('no turn is scripted');
  }
}

/// Creates a viewer thread on version 1 of [aid] as the owner, through the
/// shell's multipart route; its ID.
Future<String> _thread(HttpClaxApi api, String aid, String body) async {
  const boundary = 'clax-test-boundary';
  final parts = {
    'anchor': jsonEncode({
      'kind': 'element',
      'selector': 'body > p',
      'quote': 'Habits',
      'prefix': '',
      'suffix': '',
      'html_hash': 'sha256:00',
      'rect': null,
      'custom_name': null,
    }),
    'body': body,
    'version': '1',
  };
  final text = StringBuffer();
  for (final e in parts.entries) {
    text
      ..write('--$boundary\r\n')
      ..write('content-disposition: form-data; name="${e.key}"\r\n\r\n')
      ..write('${e.value}\r\n');
  }
  text.write('--$boundary--\r\n');
  final r = await api.transport.send(
    'POST',
    '/api/artifacts/$aid/threads',
    headers: {'content-type': 'multipart/form-data; boundary=$boundary'},
    body: Uint8List.fromList(utf8.encode(text.toString())),
    timeout: const Duration(seconds: 35),
  );
  final answer = utf8.decode(r.body);
  expect(r.status, inInclusiveRange(200, 299), reason: answer);
  return ((jsonDecode(answer) as Map)['thread'] as Map)['id'] as String;
}

/// The agent, renders and UI, which an import and a load never use.
class _Unused implements AgentPort, RenderPort, UiPort {
  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}
