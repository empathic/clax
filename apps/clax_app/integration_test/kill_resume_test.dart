// A job killed mid-publish resumes at the next launch, on macOS (spec
// 2026-10-19-in-process-core-design §7.6, S6): the debug app runs as a
// child process on a scratch CLAX_APP_DIR, its queue's worker asking a
// scripted model (CLAX_SCRIPTED_TURNS), and is killed by its PID where
// the kill hook (CLAX_TEST_KILL_AT) holds a change's publish; a second
// launch without the hook finishes the job and quits
// (CLAX_TEST_QUIT_WHEN_IDLE), and the test opens the same home to read
// what the core holds.
//
// Run: `just app-integration` (//apps/clax_app:integration_kill_resume).

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/clax/models.dart';
import 'package:clax_app/clax/sse.dart';
import 'package:clax_app/core/core_host.dart';
import 'package:clax_app/core/core_transport.dart';
import 'package:clax_app/queue/runner.dart' show EffectRunner, KillHook;
import 'package:clax_app/queue/scripted_turns.dart';
import 'package:flutter/foundation.dart' show debugPrint;
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

import 'rig.dart';

const _page = '<!doctype html><title>Goals</title><main><h2>Quarterly goals</h2></main>';
const _green =
    '<!doctype html><title>Goals</title>'
    '<main style="color:green"><h2>Quarterly goals</h2></main>';
const _reply = 'Made it green.';

/// A change in three turns: the page, a render, then finish; then, for a
/// finish whose publish was refused `not_rendered`, a render and a finish
/// again, as a model asked again would.
const _script = {
  'turns': [
    [
      {
        'type': 'tool_use',
        'id': 'w',
        'name': 'write_file',
        'input': {'path': 'index.html', 'content': _green},
      },
    ],
    [
      {'type': 'tool_use', 'id': 'r', 'name': 'render', 'input': <String, Object?>{}},
    ],
    [
      {
        'type': 'tool_use',
        'id': 'f',
        'name': 'finish',
        'input': {'reply': _reply},
      },
    ],
    [
      {'type': 'tool_use', 'id': 'r2', 'name': 'render', 'input': <String, Object?>{}},
    ],
    [
      {
        'type': 'tool_use',
        'id': 'f2',
        'name': 'finish',
        'input': {'reply': _reply},
      },
    ],
  ],
};

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  for (final at in ['after-publish-commit', 'before-publish-commit']) {
    testWidgets(
      'a change killed $at resumes at the next launch: one version, one '
      'reply',
      (tester) async {
        final dir = await Directory.systemTemp.createTemp('clax-kill-resume-');
        addTearDown(() async {
          if (Platform.environment['CLAX_KEEP_SCRATCH'] != '1') {
            await dir.delete(recursive: true);
          }
        });
        final support = Directory('${dir.path}/app/support')
          ..createSync(recursive: true);
        final script = File('${dir.path}/turns.json')
          ..writeAsStringSync(jsonEncode(_script));
        final seeded = await _seed(support);
        final child = await ChildApp.unpack(ChildApp.zip, dir);
        final env = {
          'CLAX_APP_DIR': '${dir.path}/app',
          'CLAX_APP_NO_KEY': '1',
          ScriptedTurns.variable: script.path,
        };

        // --- First launch: killed where the hook holds the publish.
        final killPoint = File('${support.path}/${KillHook.fileName}');
        final first = await child.launch({...env, KillHook.variable: at});
        await child.waitFor(first, 'the kill point', killPoint.existsSync);
        expect(killPoint.readAsStringSync(), '$at ${seeded.jobId}\n');
        Process.killPid(first.pid, ProcessSignal.sigkill);
        await first.exitCode;
        final turnsBefore = _turns(support);

        // --- Second launch: no hook; it quits once the queue is idle.
        killPoint.deleteSync();
        final second = await child.launch({
          ...env,
          'CLAX_TEST_QUIT_WHEN_IDLE': '1',
        });
        expect(await child.exited(second), 0, reason: child.tail);
        expect(killPoint.existsSync(), isFalse);

        // --- What the core holds.
        final core = _open(support);
        addTearDown(() => closeCore(core));
        final api = HttpClaxApi.over(CoreTransport(core.calls));
        final job = await _job(core, seeded.jobId);
        for (final t in _turns(support)) {
          final h = t.heard.replaceAll(RegExp(r'"data":"[^"]*"'), '"data":…');
          debugPrint('kill_resume $at: ${t.turn} heard $h');
        }
        expect(job['state'], 'done', reason: '$job; ${child.tail}');
        final detail = await api.getArtifact(seeded.aid);
        final thread = (await api.threads(seeded.aid))
            .singleWhere((t) => t.id == seeded.tid);
        final replies = [
          for (final c in thread.comments)
            if (c.authorKind == 'agent') c.body,
        ];
        final turns = _turns(support);
        debugPrint(
          'kill_resume $at: versions ${detail.versions.length}, agent '
          'replies ${replies.length}, model turns ${turns.length} '
          '(${turnsBefore.length} before the kill)',
        );
        expect(detail.versions, hasLength(2), reason: 'one version made');
        expect(
          await api.readFile(seeded.aid, 2, 'index.html'),
          _green,
        );
        expect(replies, [_reply], reason: 'one reply');
        if (at == 'after-publish-commit') {
          expect(
            [for (final t in turns) t.turn],
            ['turn 0', 'turn 1', 'turn 2'],
            reason: 'no turn is asked after a publish the core holds',
          );
        }
      },
      timeout: const Timeout(Duration(minutes: 8)),
    );
  }
}

/// The home under [support], as the app opens it.
CoreHost _open(Directory support) => CoreHost(
  home: '${support.path}/clax-home',
  supportDir: support.path,
  origins: Rig.origins,
  apiKey: () async => null,
)..open();

/// The turns the scripted model answered, across launches: each
/// `turn <index>` line of its log, with the tool results it was asked
/// with.
List<({String turn, String heard})> _turns(Directory support) {
  final f = File('${support.path}/${ScriptedTurns.logName}');
  if (!f.existsSync()) return const [];
  return [
    for (final l in f.readAsLinesSync())
      if (l.isNotEmpty)
        switch (l.split('\t')) {
          [final t, final h] => (turn: t, heard: h),
          [final t, ...] => (turn: t, heard: ''),
          _ => (turn: l, heard: ''),
        },
  ];
}

/// Makes the app the change is on (version 1), a thread on it sent to the
/// agent, and the queued change job answering it, then closes the core.
Future<({String aid, String tid, String jobId})> _seed(
  Directory support,
) async {
  Directory('${support.path}/clax-home').createSync(recursive: true);
  final core = _open(support);
  try {
    final transport = CoreTransport(core.calls);
    final api = HttpClaxApi.over(transport);
    await api.registerAppSession(
      installId: 'kill-resume-seed',
      pid: pid,
      cwd: support.path,
    );
    final made = await api.createArtifact(
      PublishRequest(title: 'Goals', files: {'index.html': _page}),
    );
    final aid = made.artifact.id;
    final tid = await _thread(transport, aid, 'make it green');
    final sent = await api.sendThread(aid, tid);
    final commentId = sent.comments.last.id;
    const jobId = 'kill-resume-1';
    await _createJob(transport, {
      'id': jobId,
      'client': 'app',
      'kind': 'change',
      'artifact_id': aid,
      'title': 'Goals',
      'input': {
        'request': 'make it green',
        'base': 1,
        'threads': {tid: commentId},
      },
    });
    return (aid: aid, tid: tid, jobId: jobId);
  } finally {
    await closeCore(core);
  }
}

/// `POST /api/artifacts/<aid>/threads` as a browser does (multipart, an
/// element anchor on version 1); the new thread's ID.
Future<String> _thread(CoreTransport t, String aid, String body) async {
  const boundary = 'clax-kill-resume';
  final anchor = jsonEncode({
    'kind': 'element',
    'selector': 'body > main > h2',
    'quote': 'Quarterly goals',
    'prefix': '',
    'suffix': '',
    'html_hash': 'sha256:00',
    'rect': null,
    'custom_name': null,
  });
  final parts = StringBuffer();
  for (final (name, value) in [
    ('anchor', anchor),
    ('body', body),
    ('version', '1'),
  ]) {
    parts
      ..write('--$boundary\r\n')
      ..write('Content-Disposition: form-data; name="$name"\r\n\r\n')
      ..write('$value\r\n');
  }
  parts.write('--$boundary--\r\n');
  final a = await t.send(
    'POST',
    '/api/artifacts/${Uri.encodeComponent(aid)}/threads',
    headers: {'content-type': 'multipart/form-data; boundary=$boundary'},
    body: Uint8List.fromList(utf8.encode(parts.toString())),
    timeout: const Duration(seconds: 30),
  );
  final json = jsonDecode(utf8.decode(a.body));
  expect(a.status, 201, reason: '$json');
  return ((json as Map)['thread'] as Map)['id'] as String;
}

/// Creates job [body] as the queue's owner does: a claim with the event
/// stream's ID, the create with its token, then the release.
Future<void> _createJob(CoreTransport t, Map<String, Object?> body) async {
  final ready = Completer<String>();
  final events = t.sse().listen((SseMessage m) {
    if (m.event != 'ready' || ready.isCompleted) return;
    if (jsonDecode(m.data) case {'stream': final String s}) ready.complete(s);
  });
  try {
    final stream = await ready.future.timeout(const Duration(seconds: 30));
    Future<Map<String, Object?>> call(
      String method,
      String path,
      Object? json, {
      String? owner,
    }) async {
      final a = await t.send(
        method,
        path,
        headers: {
          'content-type': 'application/json',
          EffectRunner.ownerHeader: ?owner,
        },
        body: json == null ? null : Uint8List.fromList(utf8.encode(jsonEncode(json))),
        timeout: const Duration(seconds: 30),
      );
      final out = a.body.isEmpty
          ? <String, Object?>{}
          : (jsonDecode(utf8.decode(a.body)) as Map).cast<String, Object?>();
      expect(a.status, lessThan(300), reason: '$method $path: $out');
      return out;
    }

    final claim = await call('POST', '/api/jobs/owner', {
      'client': 'app',
      'stream': stream,
    });
    final token = claim['owner']! as String;
    await call('POST', '/api/jobs', body, owner: token);
    await call('DELETE', '/api/jobs/owner', null, owner: token);
  } finally {
    await events.cancel();
  }
}

/// Job [id]'s row as the core holds it.
Future<Map<String, Object?>> _job(CoreHost core, String id) async {
  final a = await CoreTransport(core.calls).send(
    'GET',
    '/api/jobs/${Uri.encodeComponent(id)}',
    timeout: const Duration(seconds: 30),
  );
  final json = (jsonDecode(utf8.decode(a.body)) as Map).cast<String, Object?>();
  expect(a.status, 200, reason: '$json');
  return ((json['job']! as Map)['job']! as Map).cast<String, Object?>();
}
