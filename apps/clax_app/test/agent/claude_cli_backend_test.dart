import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/claude_cli_backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/edit_schema.dart';
import 'package:clax_app/agent/request.dart';
import 'package:flutter_test/flutter_test.dart';

/// The fixture standing in for `claude`, run through `/bin/sh`.
final fixture = File('test/fixtures/fake-claude.sh').absolute.path;

Map<String, Object?> resultJson() => {
  'reply': 'Made a run tracker.',
  'title': 'Run Tracker',
  'files': [
    {'path': 'index.html', 'content': '<!doctype html><title>Runs</title>'},
  ],
  'edits': <Object?>[],
  'removed': <Object?>[],
  'db': <Object?>[],
  'capabilities_json': '{"db":{}}',
  'resolve': true,
};

/// What `claude -p --output-format json` prints on success.
Map<String, Object?> cliOutput({
  Object? structured,
  String result = '',
  bool isError = false,
}) => {
  'type': 'result',
  'subtype': isError ? 'error_during_execution' : 'success',
  'is_error': isError,
  'result': result,
  'session_id': 'f00d',
  'structured_output': ?structured,
};

const request = AgentRequest(
  kind: RequestKind.newApp,
  model: 'claude-haiku-5-5',
  systemPrompt: 'You build small apps.\nReply only with the result.',
  userText: 'A run tracker.\n"quoted" line',
);

/// Starts the fixture with the parent's environment plus [env], and
/// records signals sent to the processes it started.
class RecordingStarter {
  RecordingStarter(this.env);

  final Map<String, String> env;
  final List<ProcessSignal> signals = [];
  final List<String> executables = [];

  Future<Process> call(
    String executable,
    List<String> arguments, {
    required String workingDirectory,
  }) async {
    executables.add(executable);
    final p = await Process.start(
      executable,
      arguments,
      workingDirectory: workingDirectory,
      environment: env,
    );
    return _SignalRecordingProcess(p, signals);
  }
}

class _SignalRecordingProcess implements Process {
  _SignalRecordingProcess(this._p, this._signals);

  final Process _p;
  final List<ProcessSignal> _signals;

  @override
  int get pid => _p.pid;
  @override
  Future<int> get exitCode => _p.exitCode;
  @override
  IOSink get stdin => _p.stdin;
  @override
  Stream<List<int>> get stdout => _p.stdout;
  @override
  Stream<List<int>> get stderr => _p.stderr;
  @override
  bool kill([ProcessSignal signal = ProcessSignal.sigterm]) {
    _signals.add(signal);
    return _p.kill(signal);
  }
}

void main() {
  late Directory tmp;
  late Directory scratch;
  late String log;

  setUp(() {
    tmp = Directory.systemTemp.createTempSync('claude_cli_test');
    scratch = Directory('${tmp.path}/scratch')..createSync();
    log = '${tmp.path}/log';
  });
  tearDown(() => tmp.deleteSync(recursive: true));

  /// A backend running the fixture with [env] (plus `FAKE_CLAUDE_LOG`).
  (ClaudeCliBackend, RecordingStarter) backend(
    Map<String, String> env, {
    Duration timeout = const Duration(seconds: 300),
  }) {
    final starter = RecordingStarter({'FAKE_CLAUDE_LOG': log, ...env});
    return (
      ClaudeCliBackend(
        executable: '/bin/sh',
        prefixArgs: [fixture],
        scratch: scratch,
        starter: starter.call,
        timeout: timeout,
      ),
      starter,
    );
  }

  String out(Object json) {
    final f = File('${tmp.path}/out.json')
      ..writeAsStringSync(json is String ? json : jsonEncode(json));
    return f.path;
  }

  Future<AgentError> fails(Future<EditResult> run) async {
    try {
      await run;
    } on AgentError catch (e) {
      return e;
    }
    fail('expected an AgentError');
  }

  test('name is claude-cli', () {
    expect(backend({}).$1.name, 'claude-cli');
  });

  test('passes the arguments in order, the request on stdin, the system '
      'prompt in a file under scratch, and runs in scratch', () async {
    final (b, starter) = backend({
      'FAKE_CLAUDE_OUT': out(cliOutput(structured: resultJson())),
    });
    await b.run(request);

    expect(starter.executables, ['/bin/sh']);
    final args = File(log).readAsLinesSync();
    final systemFile = args[8];
    expect(args, [
      '-p',
      '--output-format',
      'json',
      '--json-schema',
      jsonEncode(editSchema),
      '--tools',
      '',
      '--system-prompt-file',
      systemFile,
      '--model',
      'claude-haiku-5-5',
      '--no-session-persistence',
      '--strict-mcp-config',
    ]);
    expect(systemFile, endsWith('/system.md'));
    expect(File('$log.system').readAsStringSync(), request.systemPrompt);
    expect(File('$log.stdin').readAsStringSync(), request.userText);

    final scratchReal = scratch.resolveSymbolicLinksSync();
    final cwd = File('$log.cwd').readAsStringSync().trim();
    expect(cwd, startsWith('$scratchReal/'));
    expect(
      File(systemFile).parent.existsSync(),
      isFalse,
      reason: 'the run directory is removed after the run',
    );
    expect(scratch.listSync(), isEmpty);
  });

  test('parses structured_output into the result', () async {
    final (b, _) = backend({
      'FAKE_CLAUDE_OUT': out(
        cliOutput(structured: resultJson(), result: 'ignored'),
      ),
    });
    final r = await b.run(request);
    expect(r.toJson(), resultJson());
  });

  test('falls back to parsing result as JSON', () async {
    final (b, _) = backend({
      'FAKE_CLAUDE_OUT': out(cliOutput(result: jsonEncode(resultJson()))),
    });
    final r = await b.run(request);
    expect(r.toJson(), resultJson());
  });

  test('the result fallback accepts a fenced JSON block', () async {
    final fenced = '```json\n${jsonEncode(resultJson())}\n```';
    final (b, _) = backend({'FAKE_CLAUDE_OUT': out(cliOutput(result: fenced))});
    final r = await b.run(request);
    expect(r.toJson(), resultJson());
  });

  test('is_error is an AgentError carrying result', () async {
    final (b, _) = backend({
      'FAKE_CLAUDE_OUT': out(
        cliOutput(isError: true, result: 'Credit balance is too low'),
      ),
      'FAKE_CLAUDE_EXIT': '1',
    });
    final e = await fails(b.run(request));
    expect(e.message, contains('Credit balance is too low'));
  });

  test(
    'a non-zero exit names the exit status and the last stderr line',
    () async {
      final (b, _) = backend({
        'FAKE_CLAUDE_STDERR': 'starting\nInvalid API key · Please run /login',
        'FAKE_CLAUDE_EXIT': '1',
      });
      final e = await fails(b.run(request));
      expect(e.message, contains('exited with status 1'));
      expect(e.message, contains('Invalid API key · Please run /login'));
      expect(e.message, isNot(contains('starting')));
      expect(scratch.listSync(), isEmpty);
    },
  );

  test('a timeout kills the process and is a retryable AgentError', () async {
    final fifo = '${tmp.path}/block';
    final mk = await Process.run('mkfifo', [fifo]);
    expect(mk.exitCode, 0, reason: '${mk.stderr}');
    final (b, starter) = backend({
      'FAKE_CLAUDE_BLOCK': fifo,
      'FAKE_CLAUDE_OUT': out(cliOutput(structured: resultJson())),
    }, timeout: const Duration(milliseconds: 200));
    final e = await fails(b.run(request));
    expect(e.message, contains('did not finish'));
    expect(e.retryable, isTrue);
    expect(starter.signals, isNotEmpty);
    expect(scratch.listSync(), isEmpty);
  });

  test('output that is not JSON is a retryable AgentError', () async {
    final (b, _) = backend({'FAKE_CLAUDE_OUT': out('Hello there')});
    final e = await fails(b.run(request));
    expect(e.retryable, isTrue);
    expect(e.message, contains('not JSON'));
  });

  test('a result that is not JSON is a retryable AgentError', () async {
    final (b, _) = backend({
      'FAKE_CLAUDE_OUT': out(cliOutput(result: 'Here is your app!')),
    });
    final e = await fails(b.run(request));
    expect(e.retryable, isTrue);
  });

  test('a result of the wrong shape is a retryable AgentError naming the '
      'field', () async {
    final bad = resultJson()..remove('resolve');
    final (b, _) = backend({
      'FAKE_CLAUDE_OUT': out(cliOutput(structured: bad)),
    });
    final e = await fails(b.run(request));
    expect(e.retryable, isTrue);
    expect(e.message, contains('resolve'));
  });

  test('an executable that cannot start is an AgentError', () async {
    final b = ClaudeCliBackend(
      executable: '${tmp.path}/no-such-claude',
      scratch: scratch,
    );
    final e = await fails(b.run(request));
    expect(e.message, contains('no-such-claude'));
    expect(e.retryable, isFalse);
    expect(scratch.listSync(), isEmpty);
  });

  group('find', () {
    late Directory home;
    late Directory pathDir;

    Future<String> exe(String path, {bool executable = true}) async {
      File(path)
        ..parent.createSync(recursive: true)
        ..writeAsStringSync('#!/bin/sh\n');
      final r = await Process.run('chmod', [executable ? '755' : '644', path]);
      expect(r.exitCode, 0);
      return path;
    }

    setUp(() {
      home = Directory('${tmp.path}/home')..createSync();
      pathDir = Directory('${tmp.path}/bin')..createSync();
    });

    Map<String, String> env({String? path}) => {
      'HOME': home.path,
      'PATH': path ?? '${tmp.path}/empty:${pathDir.path}',
    };

    test('honours a configured executable first', () async {
      final configured = await exe('${tmp.path}/mine/claude');
      await exe('${pathDir.path}/claude');
      expect(
        await ClaudeCliBackend.find(configured: configured, environment: env()),
        configured,
      );
    });

    test('searches PATH when nothing usable is configured', () async {
      final onPath = await exe('${pathDir.path}/claude');
      await exe('${home.path}/.local/bin/claude');
      expect(await ClaudeCliBackend.find(environment: env()), onPath);
      expect(
        await ClaudeCliBackend.find(
          configured: '${tmp.path}/missing',
          environment: env(),
        ),
        onPath,
      );
    });

    test('falls back to ~/.local/bin/claude', () async {
      await exe('${pathDir.path}/claude', executable: false);
      final local = await exe('${home.path}/.local/bin/claude');
      expect(await ClaudeCliBackend.find(environment: env()), local);
    });

    test('is null when no claude is executable', () async {
      await exe('${pathDir.path}/claude', executable: false);
      Directory('${pathDir.path}/sub/claude').createSync(recursive: true);
      expect(
        await ClaudeCliBackend.find(
          environment: env(path: '${pathDir.path}:${pathDir.path}/sub'),
        ),
        isNull,
      );
    });
  });

  test('supported only on macOS outside the App Sandbox', () {
    expect(ClaudeCliBackend.supportedOn(macOS: true, environment: {}), isTrue);
    expect(
      ClaudeCliBackend.supportedOn(
        macOS: true,
        environment: {'APP_SANDBOX_CONTAINER_ID': 'dev.empathic.clax'},
      ),
      isFalse,
    );
    expect(
      ClaudeCliBackend.supportedOn(macOS: false, environment: {}),
      isFalse,
    );
  });
}
