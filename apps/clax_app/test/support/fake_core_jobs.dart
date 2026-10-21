import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:clax_app/bundle/content_hash.dart' show sha256Hex;
import 'package:clax_app/core/frame.dart';
import 'package:clax_app/queue/agent_port.dart' show FileHash, draftDigest;
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
///
/// The owner's live sessions are [sessions]. A publish step's
/// `session_id`, and the session a question is created from, must name
/// one (400 `unknown_session`). A reply step is
/// made from the session the job's last publish step named while it is
/// live, else from the one the reply names, and is refused
/// `unknown_session` when neither is live: as `commit_reply_step` in
/// crates/clax-store/src/jobs.rs (the route leaves an ended session it
/// names to that rule).
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

  /// Each artifact's current files by hash and size, where a test or a
  /// publish step gave them; an artifact without is taken to hold none.
  /// A publish step naming a draft must publish files whose digest
  /// (those it puts, and those it carries from here but `index.html`) is
  /// the draft's, as `next_files_digest` in crates/clax-store/src/drafts.rs
  /// checks it.
  final Map<String, Map<String, FileHash>> files = {};

  /// Each artifact's documents by path (`<collection>/<id>`) with their
  /// versions. A publish step's document write pinned (`if_version`) to
  /// another version than the document's (0 for none) refuses the whole
  /// step 409 `doc_moved` with its `path`.
  final Map<String, Map<String, int>> docs = {};

  /// Versions (`<artifact>/<n>`) whose content a killed process left with
  /// no row recording it: a publish step that would write one is 409
  /// `version_present`, as `version_present` in
  /// crates/clax-store/src/artifacts.rs refuses it.
  final Set<String> strayVersions = {};

  /// Each question the `ask` tool created: `{id, session, tool_use_id,
  /// artifact_id, source, questions, outcome, answers}`, `outcome` null
  /// while it is open. A question naming an app the core does not hold
  /// ([versions]) is 404 `not_found`.
  final Map<String, Map<String, Object?>> questions = {};

  /// The drafts a render may be recorded of; a test serving
  /// `POST /api/drafts` adds the IDs it mints.
  final Set<String> drafts = {};

  /// The renders recorded, in order: `{draft, digest, errors,
  /// failed_tests}`, `failed_tests` the count of failing tests.
  final List<Map<String, Object?>> renders = [];

  /// The bodies of every publish step committed, in order.
  final List<Map<String, Object?>> published = [];

  /// The bodies of every reply step committed, in order.
  final List<Map<String, Object?>> replied = [];

  /// The session each committed reply step was made from, in order.
  final List<String> repliedFrom = [];

  /// The owner's live sessions, which a step may name; a test registers
  /// one by adding it and ends it with [endSession].
  final Set<String> sessions = {};

  /// The session each job's last publish step was made from; none for a
  /// job whose publish step named none (an imported change), or a seeded
  /// one without `publishedFrom`.
  final Map<String, String> _publishedFrom = {};

  /// Awaited, when it answers a future, before a `PATCH /api/jobs/{id}`
  /// of job `id` with body `body` applies: a test answers a move late.
  Future<void>? Function(String id, Map<String, Object?> body)? beforeMove;

  /// Ends session [sid], as the person ending it or a revoked grant does.
  void endSession(String sid) => sessions.remove(sid);

  /// The claim of the app's queue: its token and its stream.
  String? ownerToken;
  String? ownerStream;

  /// While set, a step's commit waits for it before it is answered.
  Completer<void>? stepGate;

  /// While set, told each step's key once it commits, before its answer.
  void Function(String key)? afterStep;

  /// While set, a publish step's commit waits for it before it is
  /// answered, as a worker killed before the commit leaves it;
  /// [publishHeld] completes when the first one waits.
  Completer<void>? publishGate;
  final Completer<void> publishHeld = Completer<void>();

  /// While set, a claim answers 409 `owner_draining`.
  bool draining = false;

  /// The streams a claim subscribed to the `jobs` topic.
  final Set<String> jobStreams = {};

  int _minted = 0;
  int _seconds = 0;

  static const ownerHeader = 'x-clax-jobs-owner';

  static final _digest = RegExp(r'^sha256:[0-9a-f]{64}$');

  /// A job row in queued, as a test seeds the core with it; its publish
  /// step, if any, made from session [publishedFrom].
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
    String? publishedFrom,
  }) {
    if (publishedFrom != null) _publishedFrom[id] = publishedFrom;
    return jobs[id] = {
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
  }

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
    core.on('PATCH', '/api/jobs/{id}', (r, p) async {
      final b = _body(r, const {
        'if_rev',
        'state',
        'step',
        'title',
        'plan',
        'carry',
        'error',
      }, required: const {'if_rev'});
      // A state the core does not have is no body of the route's.
      if (b == null ||
          (b.containsKey('state') &&
              (b['state'] is! String ||
                  JobState.parse(b['state']! as String) == null))) {
        return _unprocessable();
      }
      final id = p['id']!;
      if (beforeMove?.call(id, b) case final hold?) await hold;
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
        final to = mayMove(from, MoveTo(want!), waits: _waits(_questions(id)));
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
      // A question the job holds answers its row, whatever its state.
      if (asker == id) return FakeCore.json({'job': jobs[id]});
      if (q == null || asker != null || q['source'] != 'ask') {
        return FakeCore.error(400, 'invalid_question', 'not this job\'s');
      }
      final from = JobState.parse(row(id)['state']! as String)!;
      if (from.isFinal) return _moved(id);
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
      if (publishGate case final g? when p['key']!.startsWith('publish:')) {
        if (!publishHeld.isCompleted) publishHeld.complete();
        await g.future;
      }
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
        if (pb['session_id'] case final sid?
            when sid is! String || !sessions.contains(sid)) {
          return FakeCore.error(400, 'unknown_session', 'no live session');
        }
        if (aid == null && pb['base'] != 0) {
          return FakeCore.error(
            400,
            'invalid_base',
            'a step that makes a new artifact has base 0',
          );
        }
        if (current != pb['base']) {
          return FakeCore.error(409, 'version_moved', 'moved', {
            'current': current,
          });
        }
        final next = _nextFiles(aid, pb['request']! as Map);
        if (next == null) return _unprocessable();
        if (pb['draft'] case final Map d) {
          // A later render of the same digest replaces an earlier one.
          final last = renders.lastWhere(
            (x) => x['draft'] == d['id'] && x['digest'] == d['digest'],
            orElse: () => const {},
          );
          final clean =
              last.isNotEmpty &&
              (last['errors']! as List).isEmpty &&
              last['failed_tests'] == 0;
          if (!clean || draftDigest(const {}, carried: next) != d['digest']) {
            return FakeCore.error(409, 'not_rendered_clean', 'no clean render');
          }
        }
        if (aid == null && (pb['request']! as Map)['title'] == null) {
          return FakeCore.error(400, 'invalid_args', 'a new app needs a title');
        }
        final target = aid ?? 'a${(++_minted).toString().padLeft(11, '0')}';
        made = current + 1;
        if (strayVersions.contains('$target/$made')) {
          return FakeCore.error(
            409,
            'version_present',
            'version $made of $target holds content no version records',
          );
        }
        final ops = [for (final o in (pb['docs'] as List?) ?? const []) (o as Map).cast<String, Object?>()];
        final held = docs[target] ?? const <String, int>{};
        for (final o in ops) {
          final path = o['path']! as String;
          if (o['if_version'] case final int pin when pin != (held[path] ?? 0)) {
            return FakeCore.error(409, 'doc_moved', 'document $path moved', {
              'path': path,
            });
          }
        }
        final written = docs[target] ??= {};
        for (final o in ops) {
          final path = o['path']! as String;
          if (o['op'] == 'delete') {
            written.remove(path);
          } else {
            written[path] = (written[path] ?? 0) + 1;
          }
        }
        versions[target] = made;
        files[target] = next;
        published.add(pb);
        if (pb['session_id'] case final String sid) {
          _publishedFrom[id] = sid;
        } else {
          _publishedFrom.remove(id);
        }
        result = {
          'artifact_id': target,
          'version': made,
          'docs': (pb['docs'] as List?)?.length ?? 0,
        };
        row(id)['artifact_id'] = target;
      } else if (b['reply'] case final Map rep) {
        final unknown = rep.keys.toSet().difference(const {
          'threads',
          'body',
          'resolve',
          'ack',
          'session_id',
        });
        if (unknown.isNotEmpty ||
            rep['threads'] is! List ||
            rep['body'] is! String) {
          return _unprocessable();
        }
        final named = rep['session_id'];
        if (named != null && named is! String) return _unprocessable();
        if (!steps.any((s) => (s['key']! as String).startsWith('publish:'))) {
          return FakeCore.error(400, 'invalid_step', 'no publish step');
        }
        final from = switch (_publishedFrom[id]) {
          final sid? when sessions.contains(sid) => sid,
          _ when sessions.contains(named) => named as String?,
          _ => null,
        };
        if (from == null) {
          return FakeCore.error(
            400,
            'unknown_session',
            'the version was published from no live session, and the step '
                'names none',
          );
        }
        replied.add(rep.cast());
        repliedFrom.add(from);
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
      afterStep?.call(key);
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
      if (!sessions.contains(sid)) {
        return FakeCore.error(400, 'unknown_session', 'no live session $sid');
      }
      final aid = b['artifact_id'];
      if (aid != null && (aid is! String || !versions.containsKey(aid))) {
        return FakeCore.error(404, 'not_found', 'no artifact $aid');
      }
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
        'artifact_id': aid,
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
      if (b == null ||
          b['digest'] is! String ||
          b['errors'] is! List ||
          b['failed_tests'] is! int ||
          (b['failed_tests']! as int) < 0) {
        return _unprocessable();
      }
      final digest = b['digest']! as String;
      if (!_digest.hasMatch(digest)) {
        return FakeCore.error(
          400,
          'invalid_args',
          'digest is sha256: and 64 lower-case hex digits',
        );
      }
      final id = p['id']!;
      if (!drafts.contains(id)) {
        return FakeCore.error(404, 'not_found', 'no draft $id');
      }
      final clean =
          (b['errors']! as List).isEmpty && b['failed_tests'] == 0;
      renders.add({'draft': id, ...b});
      return FakeCore.json({
        'render': {'draft_id': id, 'digest': digest, 'clean': clean},
      });
    });
  }

  // ------------------------------------------------------------ the store

  /// The files the next version of [aid] (null: a new app) holds once
  /// [request] is published on its current one: those of the current
  /// version it carries (all but `index.html` and the paths it names) and
  /// those it puts. Null for a file that is neither null nor
  /// `{content, encoding?}`.
  Map<String, FileHash>? _nextFiles(String? aid, Map request) {
    final named = (request['files'] as Map?) ?? const {};
    final out = <String, FileHash>{
      if (aid != null)
        for (final e in (files[aid] ?? const <String, FileHash>{}).entries)
          if (e.key != 'index.html' && !named.containsKey(e.key)) e.key: e.value,
    };
    for (final MapEntry(:key, :value) in named.entries) {
      if (value == null) continue;
      if (value case {'content': final String c}) {
        final bytes = value['encoding'] == 'base64'
            ? base64Decode(c)
            : utf8.encode(c);
        out['$key'] = (sha256: sha256Hex(bytes), size: bytes.length);
      } else {
        return null;
      }
    }
    return out;
  }

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
