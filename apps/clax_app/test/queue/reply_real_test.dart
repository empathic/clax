import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/render.dart';
import 'package:clax_app/queue/runner.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/real_core.dart';

/// The queue's reply step against the real embedded core: a change whose
/// version names no session (as one imported from the app's ledger) has
/// its reply made from the session the worker names.
void main() {
  test(
    'a reply step on a version that names no session is refused without '
    'the worker\'s session, and the runner\'s, which names it, commits',
    () async {
      final core = await RealCore.open();
      final api = core.api();
      final session = await api.registerAppSession(
        installId: 'install-1',
        pid: pid,
        cwd: core.dir.path,
      );
      final aid = (await api.createArtifact(
        PublishRequest(
          title: 'Habits',
          files: {'index.html': '<p>Habits</p>'},
        ),
      )).artifact.id;

      final transport = CoreTransport(core.host.calls);
      addTearDown(transport.close);
      final answers = StreamController<Msg>.broadcast();
      addTearDown(answers.close);
      final runner = EffectRunner(
        core: transport,
        agent: _Unused(),
        render: _Unused(),
        ui: _Unused(),
        probeClaude: () async => true,
        session: () async => session.id,
        send: answers.add,
      )..start();
      addTearDown(runner.close);

      /// Runs [e] and answers the runner's answer to it.
      Future<Msg> run(Effect e) {
        final answer = answers.stream.firstWhere(
          (m) => switch (m) {
            ClaimAnswer(:final effect) ||
            Committed(:final effect) ||
            Refused(:final effect) => effect == e.id,
            _ => false,
          },
        );
        runner.run(e);
        return answer.timeout(const Duration(minutes: 1));
      }

      final claimed = await run(const ClaimOwner(1));
      final token = (claimed as OwnerClaimed).token;
      final created = await run(
        CreateJob(
          2,
          'j1',
          kind: JobKind.change,
          artifactId: aid,
          input: const JobInput(request: 'say hello', base: 1),
        ),
      );
      final running = await run(
        MoveJob(3, 'j1', ifRev: (created as Committed).job.rev, to: JobState.running),
      );
      expect(running, isA<Committed>());

      Future<(int, Object?)> step(String key, Map<String, Object?> body) async {
        final a = await transport.send(
          'POST',
          '/api/jobs/j1/steps/$key',
          headers: {
            'content-type': 'application/json',
            EffectRunner.ownerHeader: token,
          },
          body: Uint8List.fromList(utf8.encode(jsonEncode(body))),
          timeout: const Duration(minutes: 1),
        );
        return (a.status, jsonDecode(utf8.decode(a.body)));
      }

      // A publish step that names no session: its version names none.
      final (published, pubBody) = await step('publish:1', {
        'publish': {
          'artifact_id': aid,
          'base': 1,
          'request': {
            'files': {
              'index.html': {'content': '<p>Habits, again</p>'},
            },
          },
        },
      });
      expect(published, 200, reason: '$pubBody');

      final (refused, refusal) = await step('reply', {
        'reply': {'threads': <String>[], 'body': 'Done.'},
        'state': 'done',
      });
      expect(refused, 400);
      expect(((refusal! as Map)['error'] as Map)['code'], 'unknown_session', reason: '$refusal');

      final replied = await run(
        const Reply(
          5,
          'j1',
          1,
          'f',
          ReplyPlan(data: {'threads': <String>[], 'body': 'Done.', 'resolve': false}),
        ),
      );
      expect(replied, isA<Committed>(), reason: '$replied');
      final c = replied as Committed;
      expect(c.job.state, JobState.done);
      expect(c.step, isA<ReplyStepResult>());
    },
    skip: RealCore.skip,
    timeout: const Timeout(Duration(minutes: 3)),
  );
}

/// The agent, renders and UI, which a reply step uses only to have the
/// agent forget the job it ended.
class _Unused implements AgentPort, RenderPort, UiPort {
  @override
  void forget(JobId jobId) {}

  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}
