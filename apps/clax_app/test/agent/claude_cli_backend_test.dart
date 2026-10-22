import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/backend.dart';
import 'package:clax_app/agent/claude_cli_backend.dart';
import 'package:clax_app/agent/edit_result.dart';
import 'package:clax_app/agent/edit_schema.dart';
import 'package:clax_app/agent/progress.dart';
import 'package:clax_app/agent/request.dart';
import 'package:flutter_test/flutter_test.dart';
import '../support/paths.dart';

/// The fixture standing in for `claude`, run through `/bin/sh`.
final fixture = File(appPath('test/fixtures/fake-claude.sh')).absolute.path;

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

/// Starts the fixture with the environment the backend passes plus
/// [extra], and records what it started and the signals sent.
class RecordingStarter {
  RecordingStarter(this.extra);

  final Map<String, String> extra;
  final List<ProcessSignal> signals = [];
  final List<String> executables = [];
  final List<Map<String, String>> environments = [];
  final List<Process> processes = [];

  Future<Process> call(
    String executable,
    List<String> arguments, {
    required String workingDirectory,
    required Map<String, String> environment,
  }) async {
    executables.add(executable);
    environments.add(environment);
    final p = await Process.start(
      executable,
      arguments,
      workingDirectory: workingDirectory,
      environment: {...environment, ...extra},
      includeParentEnvironment: false,
    );
    processes.add(p);
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
    Duration killGrace = const Duration(seconds: 5),
    Map<String, String>? parentEnvironment,
  }) {
    final starter = RecordingStarter({'FAKE_CLAUDE_LOG': log, ...env});
    return (
      ClaudeCliBackend(
        executable: '/bin/sh',
        prefixArgs: [fixture],
        scratch: scratch,
        starter: starter.call,
        timeout: timeout,
        killGrace: killGrace,
        parentEnvironment:
            parentEnvironment ?? {'PATH': Platform.environment['PATH']!},
      ),
      starter,
    );
  }

  /// A new FIFO under the test's directory.
  Future<String> fifo(String name) async {
    final path = '${tmp.path}/$name';
    final mk = await Process.run('mkfifo', [path]);
    expect(mk.exitCode, 0, reason: '${mk.stderr}');
    return path;
  }

  /// Completes once the fixture has written to the [ready] FIFO.
  Future<void> readyLine(String ready) =>
      File(ready).openRead().transform(utf8.decoder).first;

  AgentRequest cancellable(Future<void> cancelled) => AgentRequest(
    kind: request.kind,
    model: request.model,
    systemPrompt: request.systemPrompt,
    userText: request.userText,
    cancelled: cancelled,
  );

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
      '--effort',
      'low',
      '--no-session-persistence',
      '--strict-mcp-config',
      '--safe-mode',
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
    final (b, starter) = backend({
      'FAKE_CLAUDE_BLOCK': await fifo('block'),
      'FAKE_CLAUDE_OUT': out(cliOutput(structured: resultJson())),
    }, timeout: const Duration(milliseconds: 200));
    final e = await fails(b.run(request));
    expect(e.message, contains('did not finish'));
    expect(e.retryable, isTrue);
    expect(e.kind, AgentErrorKind.failed);
    expect(starter.signals, [ProcessSignal.sigterm]);
    expect(
      await starter.processes.single.exitCode,
      -ProcessSignal.sigterm.signalNumber,
    );
    expect(scratch.listSync(), isEmpty);
  });

  test('cancelling a running claude terminates it and is a cancelled, '
      'non-retryable AgentError', () async {
    final ready = await fifo('ready');
    final (b, starter) = backend({
      'FAKE_CLAUDE_READY': ready,
      'FAKE_CLAUDE_BLOCK': await fifo('block'),
      'FAKE_CLAUDE_OUT': out(cliOutput(structured: resultJson())),
    });
    final cancel = Completer<void>();
    final run = fails(b.run(cancellable(cancel.future)));
    await readyLine(ready);
    cancel.complete();
    final e = await run;
    expect(e.kind, AgentErrorKind.cancelled);
    expect(e.retryable, isFalse);
    expect(starter.signals, [ProcessSignal.sigterm]);
    expect(
      await starter.processes.single.exitCode,
      -ProcessSignal.sigterm.signalNumber,
    );
    expect(scratch.listSync(), isEmpty);
  });

  test('a claude that ignores SIGTERM gets SIGKILL after the grace and '
      'exits', () async {
    final ready = await fifo('ready');
    final (b, starter) = backend({
      'FAKE_CLAUDE_IGNORE_TERM': '1',
      'FAKE_CLAUDE_READY': ready,
      'FAKE_CLAUDE_BLOCK': await fifo('block'),
    }, killGrace: const Duration(milliseconds: 100));
    final cancel = Completer<void>();
    final run = fails(b.run(cancellable(cancel.future)));
    await readyLine(ready);
    cancel.complete();
    final e = await run;
    expect(e.kind, AgentErrorKind.cancelled);
    expect(starter.signals, [ProcessSignal.sigterm, ProcessSignal.sigkill]);
    expect(
      await starter.processes.single.exitCode,
      -ProcessSignal.sigkill.signalNumber,
    );
  });

  test('a request cancelled before the run starts no process', () async {
    final (b, starter) = backend({
      'FAKE_CLAUDE_OUT': out(cliOutput(structured: resultJson())),
    });
    final e = await fails(b.run(cancellable(Future<void>.value())));
    expect(e.kind, AgentErrorKind.cancelled);
    expect(starter.executables, isEmpty);
    expect(scratch.listSync(), isEmpty);
  });

  test('a cancellation signal that never fires leaves the run alone', () async {
    final (b, _) = backend({
      'FAKE_CLAUDE_OUT': out(cliOutput(structured: resultJson())),
    });
    final r = await b.run(cancellable(Completer<void>().future));
    expect(r.toJson(), resultJson());
  });

  test('the child gets the parent environment without the variables of a '
      'parent Claude Code session or an API key, keeping provider and auth '
      'settings', () async {
    const kept = {
      'HOME': '/Users/someone',
      'USER': 'someone',
      'TMPDIR': '/tmp/someone',
      'CLAUDE_CONFIG_DIR': '/Users/someone/.claude-work',
      'CLAUDE_CODE_OAUTH_TOKEN': 'oauth-token',
      'CLAUDE_CODE_USE_BEDROCK': '1',
      'CLAUDE_CODE_USE_VERTEX': '1',
      'CLAUDE_CODE_SKIP_BEDROCK_AUTH': '1',
      'CLAUDE_CODE_SKIP_VERTEX_AUTH': '1',
      'CLAUDE_CODE_MAX_OUTPUT_TOKENS': '32000',
      'ANTHROPIC_BASE_URL': 'https://gateway.example',
      'ANTHROPIC_MODEL': 'claude-haiku-5-5',
    };
    const dropped = {
      'CLAUDECODE': '1',
      'CLAUDE_CODE_ENTRYPOINT': 'cli',
      'CLAUDE_CODE_SESSION_ID': 'f00d',
      'CLAUDE_CODE_SESSION_ATTENDED': '1',
      'CLAUDE_CODE_CHILD_SESSION': '1',
      'CLAUDE_CODE_MESSAGING_SOCKET': '/tmp/sock',
      'CLAUDE_CODE_MESSAGING_TOKEN': 'tok',
      'CLAUDE_CODE_EXECPATH': '/usr/local/bin/claude',
      'CLAUDE_CODE_SSE_PORT': '1234',
      'CLAUDE_CODE_SIMPLE': '1',
      'CLAUDE_CODE_SAFE_MODE': '1',
      'CLAUDE_PID': '42',
      'CLAUDE_EFFORT': 'max',
      'CLAUDE_PLUGIN_DATA': '/tmp/plugin',
      'CLAUDE_PLUGIN_ROOT': '/tmp/plugin-root',
      'ANTHROPIC_API_KEY': 'sk-ant-x',
      'ANTHROPIC_AUTH_TOKEN': 'bearer-x',
    };
    final path = Platform.environment['PATH']!;
    final (b, starter) = backend(
      {'FAKE_CLAUDE_OUT': out(cliOutput(structured: resultJson()))},
      parentEnvironment: {'PATH': path, ...kept, ...dropped},
    );
    await b.run(request);
    expect(starter.environments.single, {'PATH': path, ...kept});
    final seen = {
      for (final line in File('$log.env').readAsLinesSync())
        if (line.contains('=')) line.substring(0, line.indexOf('=')),
    };
    expect(seen, containsAll(kept.keys));
    for (final name in dropped.keys) {
      expect(seen, isNot(contains(name)), reason: name);
    }
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

  group('streaming', () {
    late List<AgentProgress> events;
    setUp(() => events = []);

    AgentRequest streamed({Future<void>? cancelled, ProgressSink? sink}) =>
        AgentRequest(
          kind: request.kind,
          model: request.model,
          systemPrompt: request.systemPrompt,
          userText: request.userText,
          cancelled: cancelled,
          onProgress: sink ?? events.add,
        );

    /// One stream-json line wrapping a Messages API stream [event].
    Map<String, Object?> line(Map<String, Object?> event) => {
      'type': 'stream_event',
      'event': event,
      'session_id': 'f00d',
      'parent_tool_use_id': null,
    };

    /// The stream-json lines of a turn whose result JSON arrives in
    /// [pieces], as a text block or as a StructuredOutput tool's input.
    List<Map<String, Object?>> turn(List<String> pieces, {bool tool = false}) =>
        [
          line({
            'type': 'message_start',
            'message': {
              'id': 'msg_1',
              'content': <Object?>[],
              'usage': {'input_tokens': 9, 'output_tokens': 1},
            },
          }),
          line({
            'type': 'content_block_start',
            'index': 0,
            'content_block': tool
                ? {
                    'type': 'tool_use',
                    'id': 'toolu_1',
                    'name': 'StructuredOutput',
                    'input': <String, Object?>{},
                  }
                : {'type': 'text', 'text': ''},
          }),
          for (final p in pieces)
            line({
              'type': 'content_block_delta',
              'index': 0,
              'delta': tool
                  ? {'type': 'input_json_delta', 'partial_json': p}
                  : {'type': 'text_delta', 'text': p},
            }),
          line({'type': 'content_block_stop', 'index': 0}),
          line({
            'type': 'message_delta',
            'delta': {'stop_reason': tool ? 'tool_use' : 'end_turn'},
            'usage': {'output_tokens': 77},
          }),
          line({'type': 'message_stop'}),
        ];

    String lines(List<Object> items) =>
        [for (final i in items) i is String ? i : jsonEncode(i)]
            .map((l) => '$l\n')
            .join();

    String file(String name, String text) =>
        (File('${tmp.path}/$name')..writeAsStringSync(text)).path;

    List<String> pieces(String text, int size) => [
      for (var i = 0; i < text.length; i += size)
        text.substring(i, (i + size).clamp(0, text.length)),
    ];

    final page = '<!doctype html><title>Runs — "x"</title>${'z' * 600}';
    Map<String, Object?> result() => {
      ...resultJson(),
      'files': [
        {'path': 'index.html', 'content': page},
      ],
    };

    test('asks for stream-json with partial messages', () async {
      final (b, _) = backend({
        'FAKE_CLAUDE_OUT': out(lines([cliOutput(structured: result())])),
      });
      await b.run(streamed());
      final args = File(log).readAsLinesSync();
      expect(args, [
        '-p',
        '--output-format',
        'stream-json',
        '--verbose',
        '--include-partial-messages',
        '--json-schema',
        jsonEncode(editSchema),
        '--tools',
        '',
        '--system-prompt-file',
        args[10],
        '--model',
        'claude-haiku-5-5',
        '--effort',
        'low',
        '--no-session-persistence',
        '--strict-mcp-config',
        '--safe-mode',
      ]);
    });

    for (final tool in [false, true]) {
      test('reports the partial page from ${tool ? 'tool input' : 'text'} '
          'deltas and returns the result line\'s result', () async {
        final (b, _) = backend({
          'FAKE_CLAUDE_OUT': out(
            lines([
              {'type': 'system', 'subtype': 'init', 'session_id': 'f00d'},
              ...turn(pieces(jsonEncode(result()), 5), tool: tool),
              'not json at all',
              {'type': 'assistant', 'message': <String, Object?>{}},
              cliOutput(structured: result()),
            ]),
          ),
        });
        final r = await b.run(streamed());
        expect(r.toJson(), result());
        final texts = [
          for (final e in events)
            if (e case PartialFile(:final path, :final text))
              if (path == 'index.html') text,
        ];
        expect(texts.length, greaterThan(1));
        expect(texts.last, page);
        for (var i = 1; i < texts.length; i++) {
          expect(texts[i].startsWith(texts[i - 1]), isTrue);
        }
        expect(events.whereType<OutputTokens>().map((e) => e.count), [1, 77]);
      });
    }

    test('progress arrives while claude is still running', () async {
      final ready = await fifo('ready');
      final block = await fifo('block');
      final (b, starter) = backend({
        'FAKE_CLAUDE_EARLY': file(
          'early',
          lines(turn(pieces(jsonEncode(result()), 50)).sublist(0, 10)),
        ),
        'FAKE_CLAUDE_READY': ready,
        'FAKE_CLAUDE_BLOCK': block,
        'FAKE_CLAUDE_OUT': out(lines([cliOutput(structured: result())])),
      });
      final partial = Completer<void>();
      final run = b.run(
        streamed(
          sink: (e) {
            events.add(e);
            if (e is PartialFile && !partial.isCompleted) partial.complete();
          },
        ),
      );
      await readyLine(ready);
      await partial.future;
      var exited = false;
      unawaited(starter.processes.single.exitCode.then((_) => exited = true));
      await pumpEventQueue();
      expect(exited, isFalse);
      File(block).writeAsStringSync('go\n');
      final r = await run;
      expect(r.files.single.content, page);
    });

    test('an is_error result line is an AgentError carrying result', () async {
      final (b, _) = backend({
        'FAKE_CLAUDE_OUT': out(
          lines([cliOutput(isError: true, result: 'Credit balance too low')]),
        ),
        'FAKE_CLAUDE_EXIT': '1',
      });
      final e = await fails(b.run(streamed()));
      expect(e.message, contains('Credit balance too low'));
    });

    test('a stream without a result line is a retryable AgentError', () async {
      final (b, _) = backend({
        'FAKE_CLAUDE_OUT': out(lines(turn(['{"reply":']))),
      });
      final e = await fails(b.run(streamed()));
      expect(e.retryable, isTrue);
      expect(e.message, contains('not JSON'));
    });

    test('a claude without streaming is run again without it', () async {
      final (b, starter) = backend({
        'FAKE_CLAUDE_NO_STREAM': '1',
        'FAKE_CLAUDE_OUT': out(cliOutput(structured: result())),
      });
      final r = await b.run(streamed());
      expect(r.files.single.content, page);
      expect(starter.processes, hasLength(2));
      final args = File(log).readAsLinesSync();
      expect(args.sublist(0, 3), ['-p', '--output-format', 'json']);
      expect(args, isNot(contains('--include-partial-messages')));
      expect(events, isEmpty);
      expect(scratch.listSync(), isEmpty);
    });

    test(
      'cancelling mid-stream terminates claude and stops progress',
      () async {
        final ready = await fifo('ready');
        final (b, starter) = backend({
          'FAKE_CLAUDE_EARLY': file(
            'early',
            lines(turn(pieces(jsonEncode(result()), 50)).sublist(0, 10)),
          ),
          'FAKE_CLAUDE_READY': ready,
          'FAKE_CLAUDE_BLOCK': await fifo('block'),
        });
        final cancel = Completer<void>();
        final run = fails(b.run(streamed(cancelled: cancel.future)));
        await readyLine(ready);
        cancel.complete();
        final e = await run;
        expect(e.kind, AgentErrorKind.cancelled);
        expect(starter.signals, [ProcessSignal.sigterm]);
        final seen = events.length;
        await pumpEventQueue();
        expect(events, hasLength(seen));
      },
    );
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

    /// Only `~/.local/bin`, so this machine's Homebrew does not answer.
    List<String> localBin() => ['${home.path}/.local/bin'];

    Map<String, String> env({String? path}) => {
      'HOME': home.path,
      'PATH': path ?? '${tmp.path}/empty:${pathDir.path}',
    };

    test('honours a configured executable first', () async {
      final configured = await exe('${tmp.path}/mine/claude');
      await exe('${pathDir.path}/claude');
      expect(
        await ClaudeCliBackend.find(
          configured: configured,
          environment: env(),
          fallbacks: localBin(),
        ),
        configured,
      );
    });

    test('searches PATH when nothing usable is configured', () async {
      final onPath = await exe('${pathDir.path}/claude');
      await exe('${home.path}/.local/bin/claude');
      expect(
        await ClaudeCliBackend.find(environment: env(), fallbacks: localBin()),
        onPath,
      );
      expect(
        await ClaudeCliBackend.find(
          configured: '${tmp.path}/missing',
          environment: env(),
          fallbacks: localBin(),
        ),
        onPath,
      );
    });

    test('falls back to ~/.local/bin/claude', () async {
      await exe('${pathDir.path}/claude', executable: false);
      final local = await exe('${home.path}/.local/bin/claude');
      expect(
        await ClaudeCliBackend.find(environment: env(), fallbacks: localBin()),
        local,
      );
    });

    test('falls back to Homebrew after ~/.local/bin', () async {
      expect(ClaudeCliBackend.fallbackDirs('/Users/someone'), [
        '/Users/someone/.local/bin',
        '/opt/homebrew/bin',
        '/usr/local/bin',
      ]);
      final brew = await exe('${tmp.path}/brew/bin/claude');
      expect(
        await ClaudeCliBackend.find(
          environment: env(),
          fallbacks: ['${home.path}/.local/bin', '${tmp.path}/brew/bin'],
        ),
        brew,
      );
    });

    test('is null when no claude is executable', () async {
      await exe('${pathDir.path}/claude', executable: false);
      Directory('${pathDir.path}/sub/claude').createSync(recursive: true);
      expect(
        await ClaudeCliBackend.find(
          environment: env(path: '${pathDir.path}:${pathDir.path}/sub'),
          fallbacks: localBin(),
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
