import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'backend.dart';
import 'edit_result.dart';
import 'edit_schema.dart';
import 'request.dart';

/// Starts `claude` for [ClaudeCliBackend] with its stdin, stdout and stderr
/// piped. The child keeps the parent's environment: `claude` needs the
/// person's login from it.
typedef CliProcessStarter = Future<Process> Function(
  String executable,
  List<String> arguments, {
  required String workingDirectory,
});

/// The default [CliProcessStarter]: `Process.start` with the parent's
/// environment.
Future<Process> startWithParentEnvironment(
  String executable,
  List<String> arguments, {
  required String workingDirectory,
}) => Process.start(executable, arguments, workingDirectory: workingDirectory);

/// The desktop backend: runs `claude -p` with [editSchema] as its
/// structured output, all tools off, and the request on stdin.
///
/// Each run gets its own directory under [scratch], holding `system.md`
/// (the system prompt) and used as the working directory; it is removed
/// when the run ends. The clip is not sent: with tools off `claude` has no
/// way to read it, and the anchor and quote travel in the request text.
class ClaudeCliBackend implements AgentBackend {
  ClaudeCliBackend({
    required this.executable,
    this.prefixArgs = const [],
    required this.scratch,
    CliProcessStarter? starter,
    this.timeout = const Duration(seconds: 300),
  }) : _start = starter ?? startWithParentEnvironment;

  /// The `claude` executable (or an interpreter, with [prefixArgs]).
  final String executable;

  /// Arguments placed before `claude`'s own, such as a script for an
  /// interpreter in [executable].
  final List<String> prefixArgs;

  /// Where each run's directory is made.
  final Directory scratch;

  /// How long a run may take before it is killed.
  final Duration timeout;

  final CliProcessStarter _start;

  /// How long a timed-out run has to exit after SIGTERM before SIGKILL.
  static const killGrace = Duration(seconds: 5);

  @override
  String get name => 'claude-cli';

  /// `claude`'s arguments for a run with [model] and the system prompt in
  /// [systemPromptFile], in order.
  static List<String> arguments({
    required String model,
    required String systemPromptFile,
  }) => [
    '-p',
    '--output-format',
    'json',
    '--json-schema',
    jsonEncode(editSchema),
    '--tools',
    '',
    '--system-prompt-file',
    systemPromptFile,
    '--model',
    model,
    '--no-session-persistence',
    '--strict-mcp-config',
  ];

  /// The `claude` to run: [configured] when it is executable, else the first
  /// `claude` on `PATH`, else `~/.local/bin/claude`; null when none is.
  /// [environment] (default: this process's) supplies `PATH` and `HOME`.
  static Future<String?> find({
    String? configured,
    Map<String, String>? environment,
  }) async {
    final env = environment ?? Platform.environment;
    final home = env['HOME'] ?? '';
    final candidates = [
      if (configured != null && configured.trim().isNotEmpty)
        _expandHome(configured.trim(), home),
      for (final dir in (env['PATH'] ?? '').split(':'))
        if (dir.isNotEmpty) '$dir/claude',
      if (home.isNotEmpty) '$home/.local/bin/claude',
    ];
    for (final c in candidates) {
      if (await _isExecutable(c)) return c;
    }
    return null;
  }

  /// Whether this backend can run here: macOS outside the App Sandbox.
  static bool get supported =>
      supportedOn(macOS: Platform.isMacOS, environment: Platform.environment);

  /// [supported] for a platform: [macOS], and no `APP_SANDBOX_CONTAINER_ID`
  /// in [environment] (the sandbox sets it, and a sandboxed app may not
  /// start `claude`).
  static bool supportedOn({
    required bool macOS,
    required Map<String, String> environment,
  }) => macOS && !environment.containsKey('APP_SANDBOX_CONTAINER_ID');

  @override
  Future<EditResult> run(AgentRequest request) async {
    await scratch.create(recursive: true);
    final dir = await scratch.createTemp('run-');
    try {
      final systemFile = File('${dir.path}/system.md');
      await systemFile.writeAsString(request.systemPrompt, flush: true);
      final args = [
        ...prefixArgs,
        ...arguments(model: request.model, systemPromptFile: systemFile.path),
      ];
      final Process p;
      try {
        p = await _start(executable, args, workingDirectory: dir.path);
      } on ProcessException catch (e) {
        throw AgentError("Couldn't start claude ($executable): ${e.message}");
      }
      return await _finish(p, request.userText);
    } finally {
      try {
        await dir.delete(recursive: true);
      } on FileSystemException {
        // A leftover run directory under scratch is harmless.
      }
    }
  }

  Future<EditResult> _finish(Process p, String userText) async {
    final stdout = _collect(p.stdout);
    final stderr = _collect(p.stderr);
    // Not awaited on its own: a child that never reads stdin must still be
    // bounded by the timeout below.
    unawaited(_feed(p.stdin, userText));

    final int code;
    final List<int> out;
    final List<int> err;
    try {
      final done = await Future.wait<Object>([p.exitCode, stdout, stderr])
          .timeout(timeout);
      code = done[0] as int;
      out = done[1] as List<int>;
      err = done[2] as List<int>;
    } on TimeoutException {
      await _kill(p);
      throw AgentError(
        'claude did not finish within ${_duration(timeout)}',
        retryable: true,
      );
    }
    return _parse(
      code,
      utf8.decode(out, allowMalformed: true),
      utf8.decode(err, allowMalformed: true),
    );
  }

  static Future<void> _feed(IOSink stdin, String text) async {
    try {
      stdin.add(utf8.encode(text));
      await stdin.close();
    } on Object {
      // The child closed its stdin (it exited, or does not read it); its
      // exit status and output say what happened.
    }
  }

  static Future<List<int>> _collect(Stream<List<int>> s) => s
      .fold<BytesBuilder>(
        BytesBuilder(copy: false),
        (b, chunk) => b..add(chunk),
      )
      .then((b) => b.takeBytes());

  static Future<void> _kill(Process p) async {
    p.kill();
    try {
      await p.exitCode.timeout(killGrace);
    } on TimeoutException {
      p.kill(ProcessSignal.sigkill);
      await p.exitCode;
    }
  }

  static EditResult _parse(int code, String out, String err) {
    Object? decoded;
    try {
      decoded = jsonDecode(out);
    } on FormatException {
      decoded = null;
    }
    if (decoded is Map<String, Object?> && decoded['is_error'] == true) {
      final result = decoded['result'];
      final why = result is String && result.trim().isNotEmpty
          ? result.trim()
          : '${decoded['subtype'] ?? 'no reason given'}';
      throw AgentError('claude reported an error: ${_cut(why)}');
    }
    if (code != 0) {
      throw AgentError(
        'claude exited with status $code: ${_lastLine(err) ?? 'no message'}',
      );
    }
    if (decoded is! Map<String, Object?>) {
      throw AgentError(
        "claude's output is not JSON: ${_cut(out.trim())}",
        retryable: true,
      );
    }
    final Map<String, Object?> json;
    switch (decoded['structured_output']) {
      case final Map<String, Object?> structured:
        json = structured;
      case null:
        json = _resultJson(decoded['result']);
      case _:
        throw const AgentError(
          "claude's structured_output is not an object",
          retryable: true,
        );
    }
    try {
      return EditResult.fromJson(json);
    } on ResultInvalid catch (e) {
      throw AgentError(
        "claude's result is not an edit: ${e.reason}",
        retryable: true,
      );
    }
  }

  /// `result` parsed as a JSON object, allowing one surrounding Markdown
  /// code fence.
  static Map<String, Object?> _resultJson(Object? result) {
    if (result is! String) {
      throw const AgentError(
        'claude returned neither structured_output nor a result',
        retryable: true,
      );
    }
    var text = result.trim();
    final fence = RegExp(r'^```[a-zA-Z]*\n([\s\S]*)\n```$').firstMatch(text);
    if (fence != null) text = fence.group(1)!;
    try {
      final v = jsonDecode(text);
      if (v is Map<String, Object?>) return v;
    } on FormatException {
      // Reported below.
    }
    throw AgentError(
      "claude's result is not a JSON object: ${_cut(text)}",
      retryable: true,
    );
  }
}

Future<bool> _isExecutable(String path) async {
  final s = await FileStat.stat(path);
  return s.type == FileSystemEntityType.file && s.mode & 0x49 != 0; // 0o111
}

String _expandHome(String path, String home) =>
    path.startsWith('~/') && home.isNotEmpty
    ? '$home${path.substring(1)}'
    : path;

String? _lastLine(String text) {
  final lines = text
      .split('\n')
      .map((l) => l.trim())
      .where((l) => l.isNotEmpty);
  return lines.isEmpty ? null : _cut(lines.last);
}

/// [s] cut to 300 characters, for an error message.
String _cut(String s) => s.length <= 300 ? s : '${s.substring(0, 300)}…';

String _duration(Duration d) => d.inMilliseconds % 1000 == 0
    ? '${d.inSeconds} s'
    : '${d.inMilliseconds} ms';
