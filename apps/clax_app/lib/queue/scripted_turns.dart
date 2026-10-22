/// A debug build's model for the queue's worker, for tests that run the
/// whole app as a process (integration_test/kill_resume_test.dart).
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart' show kDebugMode;

import '../agent/backend.dart' show AgentError;
import '../agent/runner/turn.dart';
import '../agent/tools/budget.dart' show Usage;
import 'worker_main.dart' show WorkerContext;

/// Answers each turn from a script: with `CLAX_SCRIPTED_TURNS` naming a
/// JSON file `{"turns": [[block, …], …]}`, a job's turn is answered with
/// the content blocks at the index of how many turns its history already
/// holds (the assistant messages), with `stop_reason` `tool_use`. A turn
/// past the script fails with an [AgentError].
///
/// Each answer is appended to `<support>/scripted-turns.log` as a line
/// `turn <index>`, then a tab and the JSON of the tool results the turn was
/// asked with (none for the first), so a test counts the turns the model
/// was asked across launches and reads what each one heard.
final class ScriptedTurns implements TurnBackend {
  ScriptedTurns(this.turns, this.log);

  /// The environment variable naming the script.
  static const variable = 'CLAX_SCRIPTED_TURNS';

  /// The log of answered turns, under the support directory.
  static const logName = 'scripted-turns.log';

  /// Whether this build plays a script: a debug build with
  /// [variable] set.
  static bool get enabled =>
      kDebugMode && (Platform.environment[variable] ?? '').isNotEmpty;

  /// The script [variable] names, logging under [supportDir].
  factory ScriptedTurns.fromEnvironment(String supportDir) {
    final json = jsonDecode(
      File(Platform.environment[variable]!).readAsStringSync(),
    );
    final turns = switch (json) {
      {'turns': final List t} => [
        for (final blocks in t)
          [for (final b in blocks as List) (b as Map).cast<String, Object?>()],
      ],
      _ => throw const FormatException('expected {"turns": [[block, …], …]}'),
    };
    return ScriptedTurns(turns, File('$supportDir/$logName'));
  }

  final List<List<Map<String, Object?>>> turns;
  final File log;

  @override
  String get name => 'scripted';

  @override
  Future<ModelTurn> turn(TurnRequest r) async {
    final asked = r.messages.where((m) => m['role'] == 'assistant').length;
    if (asked >= turns.length) {
      throw AgentError('turn $asked is past the script');
    }
    final results = switch (r.messages.lastOrNull?['content']) {
      final List c => [
        for (final b in c)
          if (b case final Map m when m['type'] == 'tool_result') m,
      ],
      _ => const <Map>[],
    };
    log.writeAsStringSync(
      'turn $asked\t${jsonEncode(results)}\n',
      mode: FileMode.append,
      flush: true,
    );
    return ModelTurn(
      content: turns[asked],
      stopReason: 'tool_use',
      usage: const Usage(input: 100, output: 50),
    );
  }
}

/// The worker's model when [ScriptedTurns.enabled]; sendable, for
/// `workerAgent`'s `backend`.
TurnBackend scriptedTurns(WorkerContext context) =>
    ScriptedTurns.fromEnvironment(context.start.supportDir);

/// Claude counts as reachable while a script answers.
Future<bool> scriptReachable() async => true;
