import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/http_api.dart';
import 'package:clax_app/daemon/supervisor.dart' show DaemonEndpoint;
import 'package:flutter_test/flutter_test.dart';

/// [HttpClaxApi] against the real debug `clax` named by `CLAX_TEST_BIN`
/// (`tool/test-bin.sh` prints one), started on a free port with a scratch
/// home under the repository's `target/flutter-app/`, and stopped by its
/// PID in a tear-down, which runs on a failure or a timeout too.
void main() {
  final bin = Platform.environment['CLAX_TEST_BIN'];

  test(
    'the app session publishes, takes a sent comment, answers it and writes documents',
    () async {
      final d = await _Daemon.start(bin!);
      final api = HttpClaxApi(() => d.endpoint);
      addTearDown(api.close);
      final s = await api.registerAppSession(
        installId: 'install-1',
        pid: pid,
        cwd: d.dir.path,
      );
      expect(s.harness, 'app');
      final again = await api.registerAppSession(
        installId: 'install-1',
        pid: pid,
        cwd: d.dir.path,
      );
      expect(again.id, s.id, reason: 'the install ID finds the live row again');

      final created = await api.createArtifact(
        PublishRequest(
          title: 'Runs',
          capabilities: {'db': <String, Object?>{}},
          files: {'index.html': '<p>one</p>', 'app.js': 'x()'},
        ),
      );
      final aid = created.artifact.id;
      expect(created.version.n, 1);
      expect(created.artifact.ownerSessionId, s.id);
      expect(created.version.sessionId, s.id);

      final events = StreamIterator(
        api.events(artifacts: [aid], types: ['version']),
      );
      expect(await events.moveNext(), isTrue);
      expect(events.current.type, 'ready');

      final v2 = await api.publish(
        aid,
        PublishRequest(
          ifVersion: 1,
          files: {'index.html': '<p>two</p>', 'app.js': null},
        ),
      );
      expect(v2.version.n, 2);
      expect(v2.version.files.keys, ['index.html']);
      expect(await events.moveNext(), isTrue);
      expect((events.current.type, events.current.data['n']), ('version', 2));
      await events.cancel();

      await expectLater(
        api.publish(
          aid,
          PublishRequest(ifVersion: 1, files: {'index.html': '<p>stale</p>'}),
        ),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.isConflict, 'conflict', isTrue)
              .having((e) => e.current, 'current', 2),
        ),
      );

      final detail = await api.getArtifact(aid);
      expect(detail.versions.map((v) => v.n), [1, 2]);
      expect(await api.readFiles(aid, 1), {
        'index.html': '<p>one</p>',
        'app.js': 'x()',
      });
      expect(await api.readFiles(aid, 2), {'index.html': '<p>two</p>'});
      expect((await api.listArtifacts()).map((a) => a.id), contains(aid));

      // The person comments and sends it, as the shell does (no token).
      final tid = await d.viewerThread(aid, 2, 'make it blue');
      await d.viewerSend(aid, tid);

      final batch = await api.pollFeedback(
        s.id,
        wait: const Duration(seconds: 5),
      );
      final item = batch.items.single;
      expect(
        (item.artifactId, item.threadId, item.body, item.version),
        (aid, tid, 'make it blue', 2),
      );

      // Taken by the poll and not yet answered: what a restarted worker re-runs.
      var t = (await api.threads(aid)).single;
      expect(t.sentToAgent, isTrue);
      expect(t.feedbackState?.state, 'acknowledged');
      expect(t.stranded, isTrue);
      final all = await api.allThreads();
      expect(all.single.threads.single.stranded, isTrue);

      await api.setWorking(
        aid,
        on: true,
        note: 'Making it blue',
        threadIds: [tid],
      );
      await api.ackFeedback(s.id, [item.commentId]);
      await api.replyAsAgent(aid, tid, 'Done: blue now.');
      t = (await api.threads(aid)).single;
      expect(t.comments.last.byAgent, isTrue);
      expect(t.stranded, isFalse);
      await api.resolveAsAgent(aid, tid);
      t = (await api.threads(aid)).single;
      expect(t.status, 'resolved');
      expect(t.resolvedBy, 'agent:app');
      await api.setWorking(aid, on: false);
      expect(await api.clip(aid, tid), isNull);

      await api.batchDocs(aid, const [
        DocWrite(op: 'set', collection: 'runs', docId: 'r1', data: {'km': 5}),
        DocWrite(op: 'set', collection: 'runs', docId: 'r2', data: {'km': 7}),
      ]);
      final r1 = (await api.getDoc(aid, 'runs/r1'))!;
      expect(r1.data, {'km': 5});
      await api.batchDocs(aid, [
        DocWrite(
          op: 'update',
          collection: 'runs',
          docId: 'r1',
          data: {'unit': 'km'},
          ifVersion: r1.version,
        ),
      ]);
      expect((await api.getDoc(aid, 'runs/r1'))!.data, {'km': 5, 'unit': 'km'});
      expect(await api.getDoc(aid, 'runs/none'), isNull);
      expect((await api.listDocs(aid, 'runs')).map((x) => x.id), ['r1', 'r2']);
      await expectLater(
        api.batchDocs(aid, const [
          DocWrite(op: 'set', collection: 'runs', docId: 'r2', data: {}),
        ]),
        throwsA(
          isA<ClaxApiError>().having(
            (e) => e.code,
            'code',
            'if_version_required',
          ),
        ),
      );

      // Restoring version 1 brings back its files as version 3, with the
      // documents and the capability declaration untouched.
      final restored = await api.restoreVersion(aid, 1, ifVersion: 2);
      expect(restored.version.n, 3);
      expect(restored.version.label, 'Restored v1');
      expect(restored.version.files.keys.toSet(), {'index.html', 'app.js'});
      expect(await api.readFiles(aid, 3), {
        'index.html': '<p>one</p>',
        'app.js': 'x()',
      });
      expect((await api.getDoc(aid, 'runs/r1'))!.data, {'km': 5, 'unit': 'km'});
      expect((await api.getArtifact(aid)).artifact.capabilities, {
        'db': <String, Object?>{},
      });
      await expectLater(
        api.restoreVersion(aid, 1, ifVersion: 2),
        throwsA(
          isA<ClaxApiError>()
              .having((e) => e.isConflict, 'conflict', isTrue)
              .having((e) => e.current, 'current', 3),
        ),
      );
      await expectLater(
        api.restoreVersion(aid, 9, ifVersion: 3),
        throwsA(isA<ClaxApiError>().having((e) => e.isNotFound, '404', isTrue)),
      );
      expect((await api.getArtifact(aid)).versions, hasLength(3));

      await expectLater(
        api.pollFeedback('01J9NOSUCHSESSION0000000000'),
        throwsA(
          isA<ClaxApiError>().having(
            (e) => e.isUnknownSession,
            'unknown',
            isTrue,
          ),
        ),
      );
    },
    skip: bin == null ? 'CLAX_TEST_BIN is not set' : false,
    // macOS assesses a new executable on its first run, which can take a while.
    timeout: const Timeout(Duration(minutes: 3)),
  );
}

/// A daemon this test started, stopped by its PID.
class _Daemon {
  _Daemon(this.dir, this.process, this.endpoint);

  final Directory dir;
  final Process process;
  final DaemonEndpoint endpoint;

  static Future<_Daemon> start(String bin) async {
    final scratch = Directory(
      '${Directory.current.path}/../../target/flutter-app',
    )..createSync(recursive: true);
    final dir = scratch.createTempSync('clax-api-real-');
    addTearDown(() {
      if (dir.existsSync()) dir.deleteSync(recursive: true);
    });
    final home = Directory('${dir.path}/clax-home')..createSync();
    final probe = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = probe.port;
    await probe.close();
    final p = await Process.start(
      bin,
      ['--port', '$port', 'serve', '--foreground'],
      environment: {
        'CLAX_HOME': home.path,
        'HOME': dir.path,
        'TMPDIR': dir.path,
        'PATH': '/usr/bin:/bin',
        'RUST_LOG': 'warn',
      },
      includeParentEnvironment: false,
      workingDirectory: dir.path,
    );
    // Registered before anything else can fail, and run on a timeout too.
    addTearDown(() => _stop(p));
    unawaited(p.stdout.drain<void>());
    final stderr = StringBuffer();
    p.stderr.transform(utf8.decoder).listen(stderr.write);
    var exited = false;
    unawaited(p.exitCode.then((_) => exited = true));
    // Up once daemon.json names this process and /healthz answers.
    final file = File('${home.path}/daemon.json');
    final client = HttpClient();
    final deadline = DateTime.now().add(const Duration(seconds: 90));
    try {
      while (true) {
        if (exited) throw StateError('clax exited before it was up: $stderr');
        if (DateTime.now().isAfter(deadline)) {
          p.kill(ProcessSignal.sigkill);
          throw StateError('clax was not up within 90 s: $stderr');
        }
        if (file.existsSync()) {
          try {
            final j =
                jsonDecode(file.readAsStringSync()) as Map<String, Object?>;
            if (j['pid'] == p.pid) {
              final req = await client.getUrl(
                Uri.parse('http://127.0.0.1:$port/healthz'),
              );
              final res = await req.close();
              await res.drain<void>();
              if (res.statusCode == 200) {
                return _Daemon(
                  dir,
                  p,
                  DaemonEndpoint(
                    base: Uri.parse('http://127.0.0.1:$port'),
                    token: j['token'] as String,
                    pid: p.pid,
                    port: port,
                  ),
                );
              }
            }
          } on FormatException catch (_) {
            // Written but not yet complete.
          } on IOException catch (_) {
            // Not listening yet, or the connection dropped mid-answer.
          }
        }
        await Future<void>.delayed(const Duration(milliseconds: 50));
      }
    } on Object {
      p.kill(ProcessSignal.sigkill);
      rethrow;
    } finally {
      client.close(force: true);
    }
  }

  /// Creates a thread on [aid] as a viewer, through the shell's multipart
  /// route; returns its ID.
  Future<String> viewerThread(String aid, int version, String body) async {
    const boundary = 'clax-test-boundary';
    final parts = {
      'anchor': jsonEncode({
        'kind': 'element',
        'selector': 'body > p',
        'quote': 'two',
        'prefix': '',
        'suffix': '',
        'html_hash': 'sha256:00',
        'rect': null,
        'custom_name': null,
      }),
      'body': body,
      'version': '$version',
    };
    final text = StringBuffer();
    for (final e in parts.entries) {
      text
        ..write('--$boundary\r\n')
        ..write('content-disposition: form-data; name="${e.key}"\r\n\r\n')
        ..write('${e.value}\r\n');
    }
    text.write('--$boundary--\r\n');
    final j = await _viewer(
      'POST',
      '/api/artifacts/$aid/threads',
      utf8.encode(text.toString()),
      'multipart/form-data; boundary=$boundary',
    );
    return (j['thread'] as Map)['id'] as String;
  }

  /// Sends thread [tid] to the agent, as the shell's Send does.
  Future<void> viewerSend(String aid, String tid) =>
      _viewer('POST', '/api/artifacts/$aid/threads/$tid/send', const [], null);

  Future<Map<String, Object?>> _viewer(
    String method,
    String path,
    List<int> body,
    String? type,
  ) async {
    final client = HttpClient();
    try {
      final req = await client.openUrl(
        method,
        endpoint.base.replace(path: path),
      );
      if (type != null) req.headers.set(HttpHeaders.contentTypeHeader, type);
      req.contentLength = body.length;
      req.add(body);
      final res = await req.close();
      final text = await utf8.decodeStream(res);
      expect(
        res.statusCode,
        inInclusiveRange(200, 299),
        reason: '$method $path: $text',
      );
      return (jsonDecode(text) as Map).cast();
    } finally {
      client.close(force: true);
    }
  }

  /// SIGTERM, then SIGKILL when [p] has not exited within 10 s.
  static Future<void> _stop(Process p) async {
    p.kill(ProcessSignal.sigterm);
    await p.exitCode.timeout(
      const Duration(seconds: 10),
      onTimeout: () {
        p.kill(ProcessSignal.sigkill);
        return p.exitCode;
      },
    );
  }
}
