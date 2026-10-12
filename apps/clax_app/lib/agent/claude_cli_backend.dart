import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'backend.dart';
import 'edit_result.dart';
import 'edit_schema.dart';
import 'json_stream.dart';
import 'message_stream.dart';
import 'progress.dart';
import 'request.dart';

/// Starts `claude` for [ClaudeCliBackend] with its stdin, stdout and stderr
/// piped, with exactly [environment] (nothing else is inherited).
typedef CliProcessStarter = Future<Process> Function(
  String executable,
  List<String> arguments, {
  required String workingDirectory,
  required Map<String, String> environment,
});

/// The default [CliProcessStarter]: `Process.start` with exactly the given
/// environment.
Future<Process> startWithEnvironment(
  String executable,
  List<String> arguments, {
  required String workingDirectory,
  required Map<String, String> environment,
}) => Process.start(
  executable,
  arguments,
  workingDirectory: workingDirectory,
  environment: environment,
  includeParentEnvironment: false,
);

/// The environment `claude` runs with: [parent] without
/// [droppedSessionVariables] and the variables starting with one of
/// [droppedSessionPrefixes], which mark a Claude Code session the app may
/// have been started from (a child of that session would read itself as
/// one, reach its messaging socket and take its effort), and without
/// `ANTHROPIC_API_KEY` and `ANTHROPIC_AUTH_TOKEN`. Either key would replace
/// the person's Claude login, which this backend exists to use; the
/// Messages API backend is the way to run on a key. Everything else is
/// kept: `HOME`, `PATH`, `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_OAUTH_TOKEN`,
/// and provider settings such as `CLAUDE_CODE_USE_*`,
/// `CLAUDE_CODE_SKIP_*_AUTH` and `ANTHROPIC_BASE_URL`.
Map<String, String> claudeEnvironment(Map<String, String> parent) => {
  for (final MapEntry(:key, :value) in parent.entries)
    if (!_dropFromEnvironment(key)) key: value,
};

/// Variables [claudeEnvironment] drops by name.
const droppedSessionVariables = {
  'CLAUDECODE',
  'CLAUDE_PID',
  'CLAUDE_EFFORT',
  'CLAUDE_CODE_ENTRYPOINT',
  'CLAUDE_CODE_CHILD_SESSION',
  'CLAUDE_CODE_EXECPATH',
  'CLAUDE_CODE_SSE_PORT',
  'CLAUDE_CODE_SIMPLE',
  'CLAUDE_CODE_SAFE_MODE',
  'ANTHROPIC_API_KEY',
  'ANTHROPIC_AUTH_TOKEN',
};

/// Prefixes of the variables [claudeEnvironment] drops.
const droppedSessionPrefixes = [
  'CLAUDE_PLUGIN_',
  'CLAUDE_CODE_SESSION_',
  'CLAUDE_CODE_MESSAGING_',
];

bool _dropFromEnvironment(String name) =>
    droppedSessionVariables.contains(name) ||
    droppedSessionPrefixes.any(name.startsWith);

/// The desktop backend: runs `claude -p` with [editSchema] as its
/// structured output, all tools off, at effort low, and the request on
/// stdin. `--safe-mode` keeps the person's CLAUDE.md, auto-memory, skills,
/// plugins, hooks and MCP servers out of the run while their login still
/// works (`--bare` would not read the login).
///
/// Each run gets its own directory under [scratch], holding `system.md`
/// (the system prompt) and used as the working directory; it is removed
/// when the run ends. The clip is not sent: with tools off `claude` has no
/// way to read it, and the anchor and quote travel in the request text.
///
/// When the request has an [AgentRequest.onProgress] sink, `claude` runs
/// with `--output-format stream-json --verbose --include-partial-messages`:
/// the Messages API events inside its `stream_event` lines report
/// [PartialFile], [PartialEdit] and [OutputTokens] progress, whether the
/// result streams as text or as a tool's input, and its final `result`
/// line is read exactly as the `json` output is. A `claude` that refuses
/// those options (an unknown-option error before any result) is run once
/// more without streaming. Without a sink it runs with `--output-format
/// json`.
class ClaudeCliBackend implements AgentBackend {
  ClaudeCliBackend({
    required this.executable,
    this.prefixArgs = const [],
    required this.scratch,
    CliProcessStarter? starter,
    this.timeout = const Duration(seconds: 300),
    this.killGrace = const Duration(seconds: 5),
    this._parentEnvironment,
  }) : _start = starter ?? startWithEnvironment;

  /// The `claude` executable (or an interpreter, with [prefixArgs]).
  final String executable;

  /// Arguments placed before `claude`'s own, such as a script for an
  /// interpreter in [executable].
  final List<String> prefixArgs;

  /// Where each run's directory is made.
  final Directory scratch;

  /// How long a run may take before it is killed.
  final Duration timeout;

  /// How long a timed-out or cancelled run has to exit after SIGTERM
  /// before SIGKILL.
  final Duration killGrace;

  final CliProcessStarter _start;

  /// The environment [claudeEnvironment] filters; this process's when null.
  final Map<String, String>? _parentEnvironment;

  /// The effort every run asks for.
  static const effort = 'low';

  @override
  String get name => 'claude-cli';

  /// `claude`'s arguments for a run with [model] and the system prompt in
  /// [systemPromptFile], in order; [stream] asks for stream-json with
  /// partial messages.
  static List<String> arguments({
    required String model,
    required String systemPromptFile,
    bool stream = false,
  }) => [
    '-p',
    '--output-format',
    if (stream) ...[
      'stream-json',
      '--verbose',
      '--include-partial-messages',
    ] else
      'json',
    '--json-schema',
    jsonEncode(editSchema),
    '--tools',
    '',
    '--system-prompt-file',
    systemPromptFile,
    '--model',
    model,
    '--effort',
    effort,
    '--no-session-persistence',
    '--strict-mcp-config',
    '--safe-mode',
  ];

  /// The `claude` to run: [configured] when it is executable, else the first
  /// `claude` on `PATH`, else the first in [fallbacks] (default
  /// [fallbackDirs]); null when none is. [environment] (default: this
  /// process's) supplies `PATH` and `HOME`.
  static Future<String?> find({
    String? configured,
    Map<String, String>? environment,
    List<String>? fallbacks,
  }) async {
    final env = environment ?? Platform.environment;
    final home = env['HOME'] ?? '';
    final candidates = [
      if (configured != null && configured.trim().isNotEmpty)
        _expandHome(configured.trim(), home),
      for (final dir in (env['PATH'] ?? '').split(':'))
        if (dir.isNotEmpty) '$dir/claude',
      for (final dir in fallbacks ?? fallbackDirs(home))
        if (dir.isNotEmpty) '$dir/claude',
    ];
    for (final c in candidates) {
      if (await _isExecutable(c)) return c;
    }
    return null;
  }

  /// Where [find] looks after `PATH`: the native installer's
  /// `~/.local/bin`, then Homebrew's prefixes (Apple silicon, Intel). An app
  /// opened from the Finder gets a `PATH` without them.
  static List<String> fallbackDirs(String home) => [
    if (home.isNotEmpty) '$home/.local/bin',
    '/opt/homebrew/bin',
    '/usr/local/bin',
  ];

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
    final cancel = CancelWatch(request.cancelled);
    await scratch.create(recursive: true);
    cancel.check();
    final dir = await scratch.createTemp('run-');
    try {
      final systemFile = File('${dir.path}/system.md');
      await systemFile.writeAsString(request.systemPrompt, flush: true);
      Future<EditResult> attempt(ProgressSink? emit) async {
        final args = [
          ...prefixArgs,
          ...arguments(
            model: request.model,
            systemPromptFile: systemFile.path,
            stream: emit != null,
          ),
        ];
        final Process p;
        try {
          cancel.check();
          p = await _start(
            executable,
            args,
            workingDirectory: dir.path,
            environment: claudeEnvironment(
              _parentEnvironment ?? Platform.environment,
            ),
          );
        } on ProcessException catch (e) {
          throw AgentError("Couldn't start claude ($executable): ${e.message}");
        }
        return _finish(p, request.userText, cancel, emit);
      }

      final sink = request.onProgress;
      if (sink == null) return await attempt(null);
      try {
        return await attempt((event) {
          if (cancel.isCancelled) return;
          try {
            sink(event);
          } on Object {
            // A failing listener must not fail the run.
          }
        });
      } on _StreamUnsupported {
        return await attempt(null);
      }
    } finally {
      try {
        await dir.delete(recursive: true);
      } on FileSystemException {
        // A leftover run directory under scratch is harmless.
      }
    }
  }

  Future<EditResult> _finish(
    Process p,
    String userText,
    CancelWatch cancel,
    ProgressSink? emit,
  ) async {
    final streamed = emit == null ? null : _StreamedOutput(emit);
    final Future<List<int>> stdout = streamed == null
        ? _collect(p.stdout)
        : streamed.read(p.stdout);
    final stderr = _collect(p.stderr);
    // Not awaited on its own: a child that never reads stdin must still be
    // bounded by the timeout below.
    unawaited(_feed(p.stdin, userText));

    final int code;
    final List<int> out;
    final List<int> err;
    try {
      final done = await Future.any<List<Object>?>([
        Future.wait<Object>([p.exitCode, stdout, stderr]),
        cancel.whenCancelled.then((_) => null),
      ]).timeout(timeout);
      if (done == null) {
        await _kill(p);
        throw const AgentError.cancelled();
      }
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
    final errText = utf8.decode(err, allowMalformed: true);
    if (streamed != null) {
      final result = streamed.resultLine;
      if (result == null && code != 0 && _refusesStreaming(errText)) {
        throw const _StreamUnsupported();
      }
      return _parse(code, result ?? streamed.otherText, errText);
    }
    return _parse(code, utf8.decode(out, allowMalformed: true), errText);
  }

  /// Whether `claude`'s [stderr] says it does not know a streaming option.
  static bool _refusesStreaming(String stderr) {
    final text = stderr.toLowerCase();
    return text.contains('unknown option') &&
        (text.contains('--include-partial-messages') ||
            text.contains('stream-json') ||
            text.contains('--verbose'));
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

  Future<void> _kill(Process p) async {
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

/// A run in stream-json mode could not stream; it is run again without.
class _StreamUnsupported implements Exception {
  const _StreamUnsupported();
}

/// Reads `claude`'s stream-json output line by line: `stream_event` lines
/// feed a [MessageStream] whose deltas report progress, the `result` line
/// is kept as the run's answer, and lines that are neither are kept for an
/// error message.
class _StreamedOutput {
  _StreamedOutput(ProgressSink emit) {
    final blocks = BlockProgress(emit);
    _stream = MessageStream(
      onDelta: blocks.delta,
      onUsage: (n) => emit(OutputTokens(n)),
    );
  }

  late final MessageStream _stream;

  /// The last `result` line, as printed.
  String? resultLine;
  final _other = StringBuffer();

  /// The lines that were not stream events, assistant messages or a
  /// result, joined.
  String get otherText => _other.toString();

  /// Reads [stdout] to its end; returns no bytes, since the lines are kept
  /// as they are read.
  Future<List<int>> read(Stream<List<int>> stdout) async {
    await for (final line
        in stdout
            .transform(const Utf8Decoder(allowMalformed: true))
            .transform(const LineSplitter())) {
      _line(line);
    }
    return const [];
  }

  void _line(String line) {
    Object? json;
    try {
      json = jsonDecode(line);
    } on FormatException {
      json = null;
    }
    switch (json) {
      case {'type': 'stream_event', 'event': final Object? event}:
        _stream.add(event);
      case {'type': 'result'}:
        resultLine = line;
      case {'type': 'assistant' || 'user' || 'system'}:
        break;
      case _:
        if (line.trim().isNotEmpty) _other.writeln(line);
    }
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
