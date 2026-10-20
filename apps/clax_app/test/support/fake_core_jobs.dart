import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/core/frame.dart';
import 'package:clax_app/queue/job_moves.dart';

import 'fake_core.dart';

/// The core's job routes on a [FakeCore], answering as
/// crates/clax-server/src/routes/jobs.rs and docs/contract.md ("Jobs")
/// say, in their JSON: the queue owner's claim and release, `POST
/// /api/jobs`, `GET /api/jobs?open=1&client=…`, `GET /api/jobs/{id}`,
/// `PATCH /api/jobs/{id}`, `PUT /api/jobs/{id}/questions/{qid}` and
/// `POST /api/jobs/{id}/steps/{key}`; with them the question creation the
/// `ask` tool makes (`POST /api/sessions/{sid}/questions`, the same
/// `tool_use_id` from the same session answering the first question) and
/// the render record of spec 2026-10-19-in-process-core-design §8.2
/// (`POST /api/drafts/{id}/renders`).
///
/// Each body is read as the routes read it: an unknown field is 422, as
/// `deny_unknown_fields` refuses it. Every write of a job needs the
/// owner's token in `x-clax-jobs-owner` (409 `not_owner`), moves its `rev`
/// one on, and sends `event: job` to every stream a claim subscribed to
/// the `jobs` topic; each claim and release sends `event: jobs_owner`.
/// Moves follow [mayMove], the store's table.
class FakeJobs {
  FakeJobs(this.core) {
    _routes();
  }

  final FakeCore core;

  /// Each job as `GET /api/jobs/{id}` answers it (`{job, steps, threads,
  /// questions}`), oldest first.
  final Map<String, Map<String, Object?>> jobs = {};

  /// Each artifact's current version.
  final Map<String, int> versions = {};

  /// Each question the `ask` tool created: `{id, session, tool_use_id,
  /// outcome, answers}`, `outcome` null while it is open.
  final Map<String, Map<String, Object?>> questions = {};

  /// The renders recorded, in order: `{draft, digest, errors,
  /// failed_tests}`.
  final List<Map<String, Object?>> renders = [];

  /// The bodies of every publish step committed, in order.
  final List<Map<String, Object?>> published = [];

  /// The bodies of every reply step committed, in order.
  final List<Map<String, Object?>> replied = [];

  /// The claim of the app's queue: its token and its stream.
  String? ownerToken;
  String? ownerStream;

  /// While set, a step's commit waits for it before it is answered.
  Completer<void>? stepGate;

  /// While set, a claim answers 409 `owner_draining`.
  bool draining = false;

  /// The streams a claim subscribed to the `jobs` topic.
  final Set<String> jobStreams = {};

  int _minted = 0;
  int _seconds = 0;

  static const ownerHeader = 'x-clax-jobs-owner';

  /// A job row in queued, as a test seeds the core with it.
  Map<String, Object?> seed(
    String id, {
    String kind = 'make',
    String state = 'queued',
    String? step,
    int rev = 1,
    String? artifactId,
    String? title,
    Map<String, Object?> input = const {'request': 'a timer'},
    Object? plan,
    Object? carry,
    Object? error,
    List<Map<String, Object?>> steps = const [],
    List<Map<String, Object?>> questions = const [],
  }) => jobs[id] = {
    'job': {
      'id': id,
      'client': 'app',
      'kind': kind,
      'artifact_id': artifactId,
      'state': state,
      'step': step,
      'title': title,
      'input': input,
      'plan': plan,
      'carry': carry,
      'error': error,
      'rev': rev,
      'created_at': _now(),
      'updated_at': _now(),
    },
    'steps': steps,
    'threads': const <Object?>[],
    'questions': questions,
  };

  /// Job [id]'s row (`job`).
  Map<String, Object?> row(String id) =>
      (jobs[id]!['job']! as Map).cast<String, Object?>();

  /// The stream holding the claim ends: the claim is released, as the core
  /// releases it when the claimant's connection ends.
  void dropOwnerStream() {
    final s = ownerStream;
    if (s == null) return;
    core.stream(s)?.end('stream_ended');
    _release();
  }

  /// Question [qid] closes with [outcome] (`answered` with [answers],
  /// `declined`, `withdrawn`): its job stores it and moves back to
  /// running once it waits on none, as the question's close does.
  void closeQuestion(String qid, String outcome, [Object? answers]) {
    final q = questions[qid]!;
    q['outcome'] = outcome;
    q['answers'] = answers;
    for (final id in jobs.keys) {
      final full = jobs[id]!;
      final qs = _list(full['questions']);
      final i = qs.indexWhere((e) => e['question_id'] == qid);
      if (i < 0) continue;
      final job = row(id);
      final state = JobState.parse(job['state']! as String)!;
      if (state.isFinal) return;
      qs[i] = {...qs[i], 'answer': _answer(q)};
      full['questions'] = qs;
      final to = mayMove(state, const MoveClose(), waits: _waits(qs))!;
      _write(id, {'state': to.name});
    }
  }

  /// Another writer publishes a version of [aid].
  void publishElsewhere(String aid) =>
      versions[aid] = (versions[aid] ?? 0) + 1;

  // ------------------------------------------------------------ routes

  void _routes() {
    core.on('POST', '/api/jobs/owner', (r, _) {
      final b = _body(r, const {'client', 'stream'});
      if (b == null) return _unprocessable();
      final stream = b['stream']! as String;
      if (core.stream(stream) == null) {
        return FakeCore.error(
          400,
          'no_such_stream',
          '$stream is not a live stream',
        );
      }
      jobStreams.add(stream);
      if (ownerStream case final held? when core.stream(held) == null) {
        _release();
      }
      if (ownerStream case final held? when held != stream) {
        return FakeCore.error(409, 'owner_held', 'held by another stream');
      }
      if (draining) {
        return FakeCore.error(409, 'owner_draining', 'writes still run');
      }
      if (ownerStream == stream) return FakeCore.json({'owner': ownerToken});
      ownerStream = stream;
      ownerToken = (++_minted).toRadixString(16).padLeft(32, '0');
      _event('jobs_owner', {'client': 'app', 'held': true});
      return FakeCore.json({'owner': ownerToken});
    });
    core.on('DELETE', '/api/jobs/owner', (r, _) {
      // The claim the token holds, whether or not its stream lives.
      final token = r.headers
          .where((h) => h.$1.toLowerCase() == ownerHeader)
          .map((h) => h.$2)
          .firstOrNull;
      if (token != null && token == ownerToken) _release();
      return CoreResponse(204, const [], Uint8List(0));
    });
    core.on('POST', '/api/jobs', (r, _) {
      final b = _body(r, const {
        'id',
        'client',
        'kind',
        'artifact_id',
        'title',
        'input',
        'plan',
      }, required: const {'id', 'client', 'kind', 'input'});
      if (b == null) return _unprocessable();
      if (!_owner(r)) return _notOwner();
      final id = b['id']! as String;
      final aid = b['artifact_id'] as String?;
      if (jobs[id] case final held?) {
        final j = (held['job']! as Map).cast<String, Object?>();
        final same =
            j['kind'] == b['kind'] &&
            j['artifact_id'] == aid &&
            j['title'] == b['title'] &&
            _jsonEq(j['input'], b['input']) &&
            _jsonEq(j['plan'], b['plan']);
        return same
            ? FakeCore.json({'job': held})
            : FakeCore.error(409, 'job_exists', 'another body');
      }
      if (aid != null && !versions.containsKey(aid)) {
        return FakeCore.error(400, 'invalid_artifact', 'no artifact $aid');
      }
      seed(
        id,
        kind: b['kind']! as String,
        artifactId: aid,
        title: b['title'] as String?,
        input: (b['input']! as Map).cast(),
        plan: b['plan'],
        rev: 0,
      );
      _write(id, const {});
      return FakeCore.json({'job': jobs[id]}, status: 201);
    });
    core.on('GET', '/api/jobs', (r, _) {
      final q = Uri.parse(r.path).queryParameters;
      if (q['open'] != '1' || q['client'] == null) {
        return FakeCore.error(400, 'invalid_query', 'open=1&client=<c>');
      }
      return FakeCore.json({
        'jobs': [
          for (final full in jobs.values)
            if (_isOpen(full) && (full['job']! as Map)['client'] == q['client'])
              full,
        ],
      });
    });
    core.on('GET', '/api/jobs/{id}', (r, p) {
      final full = jobs[p['id']];
      if (full == null) return FakeCore.error(404, 'not_found', 'no job');
      return FakeCore.json({'job': full});
    });
    core.on('PATCH', '/api/jobs/{id}', (r, p) {
      final b = _body(r, const {
        'if_rev',
        'state',
        'step',
        'title',
        'plan',
        'carry',
        'error',
      }, required: const {'if_rev'});
      if (b == null) return _unprocessable();
      final id = p['id']!;
      if (!jobs.containsKey(id)) return FakeCore.error(404, 'not_found', '');
      if (!_owner(r)) return _notOwner();
      final job = row(id);
      final from = JobState.parse(job['state']! as String)!;
      if (from.isFinal || job['rev'] != b['if_rev']) return _moved(id);
      final patch = <String, Object?>{
        for (final k in const ['step', 'title', 'plan', 'carry', 'error'])
          if (b.containsKey(k)) k: b[k],
      };
      if (b['state'] case final String s) {
        final want = JobState.parse(s);
        final to = want == null
            ? null
            : mayMove(from, MoveTo(want), waits: _waits(_questions(id)));
        if (to == null) {
          return FakeCore.error(400, 'invalid_move', '$from to $s');
        }
        patch['state'] = to.name;
      }
      return FakeCore.json({'job': _write(id, patch)});
    });
    core.on('PUT', '/api/jobs/{id}/questions/{qid}', (r, p) {
      final id = p['id']!;
      final qid = p['qid']!;
      if (!jobs.containsKey(id)) return FakeCore.error(404, 'not_found', '');
      if (!_owner(r)) return _notOwner();
      final q = questions[qid];
      final asker = jobs.entries
          .where(
            (e) => _list(e.value['questions']).any(
              (x) => x['question_id'] == qid,
            ),
          )
          .map((e) => e.key)
          .firstOrNull;
      if (q == null || (asker != null && asker != id)) {
        return FakeCore.error(400, 'invalid_question', 'not this job\'s');
      }
      final from = JobState.parse(row(id)['state']! as String)!;
      if (from.isFinal) return _moved(id);
      if (asker == id) return FakeCore.json({'job': jobs[id]});
      final qs = [
        ..._questions(id),
        {
          'question_id': qid,
          'tool_use_id': q['tool_use_id'],
          'answer': _answer(q),
        },
      ];
      jobs[id]!['questions'] = qs;
      final to = mayMove(from, const MoveAsk(), waits: _waits(qs))!;
      return FakeCore.json({
        'job': _write(id, {'state': to.name}),
      });
    });
    core.on('POST', '/api/jobs/{id}/steps/{key}', (r, p) async {
      if (stepGate case final g?) await g.future;
      final b = _body(r, const {'result', 'publish', 'reply', 'state'});
      if (b == null) return _unprocessable();
      final id = p['id']!;
      final key = p['key']!;
      final done = switch (b['state']) {
        null => false,
        'done' => true,
        _ => null,
      };
      if (done == null) {
        return FakeCore.error(400, 'invalid_move', 'a step ends done');
      }
      final kinds = [
        for (final k in const ['publish', 'reply'])
          if (b[k] != null) k,
      ];
      if (kinds.length > 1 || (kinds.isNotEmpty && b['result'] != null)) {
        return FakeCore.error(400, 'invalid_step', 'one of them');
      }
      if (!jobs.containsKey(id)) return FakeCore.error(404, 'not_found', '');
      if (!_owner(r)) return _notOwner();
      final steps = _list(jobs[id]!['steps']);
      if (steps.where((s) => s['key'] == key).firstOrNull case final s?) {
        return FakeCore.json({
          'job': jobs[id],
          'step': s,
          'replayed': true,
          if (s['version_n'] != null) 'version': s['version_n'],
        });
      }
      final from = JobState.parse(row(id)['state']! as String)!;
      if (from.isFinal) return _moved(id);
      Object? result = b['result'];
      int? made;
      if (b['publish'] case final Map pub) {
        final pb = pub.cast<String, Object?>();
        final unknown = pb.keys.toSet().difference(const {
          'artifact_id',
          'base',
          'request',
          'docs',
          'draft',
          'session_id',
        });
        if (unknown.isNotEmpty || pb['request'] is! Map || pb['base'] is! int) {
          return _unprocessable();
        }
        final aid = pb['artifact_id'] as String?;
        final current = aid == null ? 0 : (versions[aid] ?? 0);
        if (current != pb['base']) {
          return FakeCore.error(409, 'version_moved', 'moved', {
            'current': current,
          });
        }
        if (pb['draft'] case final Map d) {
          final clean = renders.any(
            (x) =>
                x['draft'] == d['id'] &&
                x['digest'] == d['digest'] &&
                (x['errors'] as List).isEmpty &&
                (x['failed_tests'] as List).isEmpty,
          );
          if (!clean) {
            return FakeCore.error(409, 'not_rendered_clean', 'no clean render');
          }
        }
        if (aid == null && (pb['request']! as Map)['title'] == null) {
          return FakeCore.error(400, 'invalid_args', 'a new app needs a title');
        }
        final target = aid ?? 'a${(++_minted).toString().padLeft(11, '0')}';
        made = current + 1;
        versions[target] = made;
        published.add(pb);
        result = {
          'artifact_id': target,
          'version': made,
          'docs': (pb['docs'] as List?)?.length ?? 0,
        };
        row(id)['artifact_id'] = target;
      } else if (b['reply'] case final Map rep) {
        if (!steps.any((s) => (s['key']! as String).startsWith('publish:'))) {
          return FakeCore.error(400, 'invalid_step', 'no publish step');
        }
        replied.add(rep.cast());
        result = {
          'comments': [
            for (final t in rep['threads'] as List)
              {'thread_id': t, 'comment_id': 'c-$t'},
          ],
          'resolved': rep['resolve'] == true,
        };
      }
      final step = {
        'key': key,
        'result': result,
        'version_n': made,
        'created_at': _now(),
      };
      jobs[id]!['steps'] = [...steps, step];
      final to = mayMove(
        from,
        MoveStep(done: done),
        waits: _waits(_questions(id)),
      )!;
      return FakeCore.json({
        'job': _write(id, {'step': key, 'error': null, 'state': to.name}),
        'step': step,
        'replayed': false,
        'version': ?made,
      });
    });
    core.on('POST', '/api/sessions/{sid}/questions', (r, p) {
      final b = _body(r, const {
        'questions',
        'artifact_id',
        'thread_id',
        'in_terminal',
        'source',
        'tool_use_id',
      }, required: const {'questions', 'source'});
      if (b == null) return _unprocessable();
      final sid = p['sid']!;
      final tool = b['tool_use_id'] as String?;
      if (tool != null) {
        final held = questions.values
            .where((q) => q['session'] == sid && q['tool_use_id'] == tool)
            .firstOrNull;
        if (held != null) return FakeCore.json(_asked(held));
      }
      final q = <String, Object?>{
        'id': 'q${++_minted}',
        'session': sid,
        'tool_use_id': tool,
        'source': b['source'],
        'questions': b['questions'],
        'outcome': null,
        'answers': null,
      };
      questions[q['id']! as String] = q;
      return FakeCore.json(_asked(q), status: 201);
    });
    core.on('POST', '/api/drafts/{id}/renders', (r, p) {
      final b = _body(r, const {
        'digest',
        'errors',
        'failed_tests',
      }, required: const {'digest', 'errors', 'failed_tests'});
      if (b == null) return _unprocessable();
      renders.add({'draft': p['id'], ...b});
      return CoreResponse(204, const [], Uint8List(0));
    });
  }

  // ------------------------------------------------------------ the store

  /// Writes [patch] to job [id]'s row, moving its `rev` on; a final move
  /// withdraws its open questions. Sends `event: job`; answers the job.
  Map<String, Object?> _write(String id, Map<String, Object?> patch) {
    final job = row(id);
    job.addAll(patch);
    job['rev'] = (job['rev']! as int) + 1;
    job['updated_at'] = _now();
    if (JobState.parse(job['state']! as String)!.isFinal) {
      jobs[id]!['questions'] = [
        for (final q in _questions(id))
          (q['answer'] as Map?)?['outcome'] == 'pending'
              ? {
                  ...q,
                  'answer': {'outcome': 'withdrawn'},
                }
              : q,
      ];
    }
    _event('job', {
      'id': id,
      'client': job['client'],
      'state': job['state'],
      'step': job['step'],
      'rev': job['rev'],
    });
    return jobs[id]!;
  }

  void _release() {
    ownerStream = null;
    ownerToken = null;
    _event('jobs_owner', {'client': 'app', 'held': false});
  }

  void _event(String name, Map<String, Object?> data) {
    final block =
        'event: $name\ndata: ${jsonEncode({'type': name, ...data})}\n\n';
    for (final s in jobStreams) {
      core.stream(s)?.push(block);
    }
  }

  bool _owner(CoreRequest r) {
    final token = r.headers
        .where((h) => h.$1.toLowerCase() == ownerHeader)
        .map((h) => h.$2)
        .firstOrNull;
    final held = ownerStream;
    return token != null &&
        token == ownerToken &&
        held != null &&
        core.stream(held) != null;
  }

  CoreResponse _moved(String id) =>
      FakeCore.error(409, 'job_moved', 'job $id moved', {'job': jobs[id]});

  static CoreResponse _notOwner() =>
      FakeCore.error(409, 'not_owner', 'claim it with POST /api/jobs/owner');

  static CoreResponse _unprocessable() =>
      FakeCore.error(422, 'invalid_json', 'the body is not the route\'s');

  /// [r]'s JSON object when its fields are among [allowed] and include
  /// [required]; null otherwise.
  static Map<String, Object?>? _body(
    CoreRequest r,
    Set<String> allowed, {
    Set<String> required = const {},
  }) {
    final b = FakeCore.bodyOf(r);
    if (b is! Map) return null;
    final m = b.cast<String, Object?>();
    if (!allowed.containsAll(m.keys)) return null;
    if (!m.keys.toSet().containsAll(required)) return null;
    return m;
  }

  List<Map<String, Object?>> _questions(String id) =>
      _list(jobs[id]!['questions']);

  static List<Map<String, Object?>> _list(Object? v) => [
    for (final e in (v as List?) ?? const []) (e as Map).cast(),
  ];

  static bool _waits(List<Map<String, Object?>> qs) => qs.any(
    (q) => (q['answer'] as Map?)?['outcome'] == 'pending',
  );

  static bool _isOpen(Map<String, Object?> full) {
    final s = (full['job']! as Map)['state'];
    return s != 'cancelled' && s != 'done';
  }

  static Map<String, Object?> _answer(Map<String, Object?> q) =>
      switch (q['outcome']) {
        null => {'outcome': 'pending'},
        'answered' => {'outcome': 'answered', 'answers': q['answers']},
        final String o => {'outcome': o},
        final o => throw StateError('an outcome: $o'),
      };

  static Map<String, Object?> _asked(Map<String, Object?> q) => {
    'question': {
      'id': q['id'],
      'status': q['outcome'] ?? 'open',
      'tool_use_id': q['tool_use_id'],
      'source': q['source'],
      'questions': q['questions'],
    },
    'mode': 'wait',
    'terminal_after_s': 0,
    'surface_open': true,
    'where_to_ask': 'app',
  };

  static bool _jsonEq(Object? a, Object? b) => jsonEncode(a) == jsonEncode(b);

  String _now() =>
      DateTime.utc(2026, 10, 20).add(Duration(seconds: _seconds++)).toIso8601String();
}
