/// The plan of a make the worker isolate's queue finished, handed to the
/// UI isolate's plans and build worker, so the rest of it is built as the
/// make run in this isolate builds it (`AppRoot._make` in lib/app.dart).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import '../agent/plan.dart';
import '../agent/pricing.dart';
import '../clax/transport.dart';
import 'agent_port.dart' show AgentTurns;
import 'client.dart';
import 'model.dart' show Job, TurnStepResult, planOf;
import 'runner.dart' show jobOf;

/// What job [j]'s committed turns cost together, in US dollars: each
/// turn priced as `RunBudget.addTurn` prices it, per attempt at the
/// attempt's model, or, for a turn its notes name no model of, at [model].
double spentOf(Job j, String model) {
  var usd = 0.0;
  for (final r in j.steps.values) {
    if (r is! TurnStepResult) continue;
    final (usage, attempts) = AgentTurns.usageOf(r.notes);
    if (attempts.isEmpty) {
      usd += costOf(model, usage);
    } else {
      for (final a in attempts) {
        usd += costOf(a.model, a.usage);
      }
    }
  }
  return usd;
}

/// Job [id]'s row, read over [transport] (`GET /api/jobs/{id}`); null when
/// the core holds none. Throws when the core gave no answer, or one that
/// is not a job.
Future<Job?> readJobOver(ClaxTransport transport, JobId id) async {
  final a = await transport.send(
    'GET',
    '/api/jobs/${Uri.encodeComponent(id)}',
    timeout: const Duration(seconds: 35),
  );
  if (a.status == 404) return null;
  final json = a.body.isEmpty ? null : jsonDecode(utf8.decode(a.body));
  if (a.status == 200 && json is Map) {
    if (jobOf(json['job']) case final job?) return job;
  }
  throw StateError('job $id could not be read (${a.status})');
}

/// The makes the queue holds whose plans are not yet handed over, kept in
/// [file] (a JSON list of job IDs, written to a temporary file beside it
/// and renamed into place) or, without one, in memory, so a make that
/// finished while no UI isolate heard it is handed over at the next
/// launch. A file that cannot be read is treated as empty.
class MakeHandOffLedger {
  MakeHandOffLedger({this.file});

  final File? file;
  final _ids = <String>{};
  Future<void>? _loaded;
  Future<void> _saving = Future.value();

  Future<void> _load() => _loaded ??= () async {
    final f = file;
    if (f == null) return;
    try {
      if (!await f.exists()) return;
      for (final id in jsonDecode(await f.readAsString()) as List) {
        if (id is String) _ids.add(id);
      }
    } on Object {
      // An unreadable file loses the hand-overs still to make.
    }
  }();

  Future<void> _save() {
    final f = file;
    if (f == null) return Future.value();
    final json = jsonEncode([..._ids]);
    return _saving = _saving
        .then((_) async {
          await f.parent.create(recursive: true);
          final tmp = File('${f.path}.tmp');
          await tmp.writeAsString(json, flush: true);
          await tmp.rename(f.path);
        })
        .catchError((Object _) {});
  }

  /// The makes kept.
  Future<Set<String>> all() async {
    await _load();
    return {..._ids};
  }

  /// Keeps make [id].
  Future<void> add(String id) async {
    await _load();
    if (_ids.add(id)) await _save();
  }

  /// Forgets make [id].
  Future<void> remove(String id) async {
    await _load();
    if (_ids.remove(id)) await _save();
  }
}

/// Hands each make the queue finished to [plans] and [startBuild]: its
/// plan ([planOf], replayed from its row's committed turns, read with
/// [readJob]) is kept as the new app's, with what the make's turns cost
/// ([spentOf] at [model]) as the build's spend, so the build counts it
/// against `buildCapUsd`; and when the plan has features left to build
/// and is not stopped, the app's build is started. A plan the app already
/// has is never written over, and no build is started for it.
///
/// Each make the queue shows not yet final is kept in [ledger] until it is
/// handed over or ends without an app: a make that finished while no UI
/// isolate heard the queue is handed over once [queue] shows it done, or,
/// once the queue no longer holds it, from its row.
class MakeHandOff {
  MakeHandOff({
    required this.queue,
    required this.plans,
    required this.readJob,
    required this.startBuild,
    required this.model,
    MakeHandOffLedger? ledger,
    void Function(String line)? log,
  }) : ledger = ledger ?? MakeHandOffLedger(),
       _log = log ?? ((_) {});

  final QueueClient queue;
  final PlanStore plans;

  /// Job [id]'s row; null when the core holds none. Throws when it could
  /// not be read: the make is tried again at the queue's next state.
  final Future<Job?> Function(JobId id) readJob;
  final void Function(String appId) startBuild;

  /// The model a turn whose notes name none ran on.
  final String Function() model;
  final MakeHandOffLedger ledger;
  final void Function(String line) _log;

  /// The makes being handed over now, and those handed over or ended in
  /// this isolate's life.
  final _busy = <JobId>{};
  final _over = <JobId>{};
  var _started = false;
  var _closed = false;

  /// Hears the queue's states, the one it shows now first.
  void start() {
    if (_started) return;
    _started = true;
    queue.state.addListener(_onState);
    _onState();
  }

  void dispose() {
    _closed = true;
    if (_started) queue.state.removeListener(_onState);
  }

  void _onState() => _track(_check(queue.state.value));

  /// The work started and not yet over.
  final _work = <Future<void>>{};

  void _track(Future<void> f) {
    _work.add(f);
    unawaited(f.whenComplete(() => _work.remove(f)));
  }

  /// Completes once the work the states heard so far started is over,
  /// its writes to [plans] and [ledger] included.
  Future<void> settled() async {
    while (_work.isNotEmpty) {
      await Future.wait([..._work]);
    }
  }

  Future<void> _check(QueueState s) async {
    final shown = <JobId>{};
    for (final j in s.jobs) {
      if (j.kind != JobKind.make) continue;
      shown.add(j.id);
      if (!j.state.isFinal) {
        if (!_over.contains(j.id)) _track(ledger.add(j.id));
      } else {
        _track(_handOver(j.id));
      }
    }
    if (!s.loaded) return;
    // A kept make the loaded queue no longer holds ended meanwhile.
    for (final id in await ledger.all()) {
      if (!shown.contains(id)) _track(_handOver(id));
    }
  }

  Future<void> _handOver(JobId id) async {
    if (_closed || _over.contains(id) || !_busy.add(id)) return;
    try {
      final Job? row;
      try {
        row = await readJob(id);
      } on Object catch (e) {
        _log('make $id: its row could not be read: $e');
        return;
      }
      if (row != null && !row.state.isFinal) return;
      final aid = row?.artifactId;
      if (row != null && row.state == JobState.done && aid != null) {
        await _adopt(row, aid);
      }
      _over.add(id);
      await ledger.remove(id);
    } on Object catch (e) {
      _log('make $id: handing its plan over failed: $e');
    } finally {
      _busy.remove(id);
    }
  }

  Future<void> _adopt(Job row, String aid) async {
    final planned = planOf(row);
    if (planned == null) return;
    final kept = await plans.adopt(
      aid,
      [
        for (final f in planned)
          PlanFeature(
            id: f.id,
            title: f.title,
            detail: f.detail,
            status: FeatureStatus.values.byName(f.status),
          ),
      ],
      request: row.input.request,
      spentUsd: spentOf(row, model()),
    );
    if (!kept || _closed) return;
    final plan = await plans.get(aid);
    if (plan != null && !plan.stopped && plan.todo.isNotEmpty) {
      startBuild(aid);
    }
  }
}
