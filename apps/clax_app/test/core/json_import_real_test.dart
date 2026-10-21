import 'dart:async';
import 'dart:io';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/core/json_import.dart';
import 'package:clax_app/queue/effect.dart';
import 'package:clax_app/queue/model.dart';
import 'package:clax_app/queue/msg.dart';
import 'package:clax_app/queue/render.dart';
import 'package:clax_app/queue/runner.dart';
import 'package:flutter_test/flutter_test.dart';

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
}

/// The agent, renders and UI, which an import and a load never use.
class _Unused implements AgentPort, RenderPort, UiPort {
  @override
  dynamic noSuchMethod(Invocation invocation) =>
      throw UnimplementedError('${invocation.memberName}');
}
