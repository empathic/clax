/// What a live build eval runs: the model, the prompts, the caps and the
/// spend guard. Pure Dart (no Flutter), so `preflight.dart` can check a
/// run with `dart run` before anything is built.
library;

import 'dart:convert';
import 'dart:io';

import 'package:clax_app/agent/pricing.dart';
import 'package:clax_app/platform/settings_store.dart';

/// The most one build may spend: the build cap of a plan's runs
/// (`buildCapUsd` in `lib/agent/plan.dart`; a test keeps the two equal).
const perBuildCapUsd = 6.0;

/// One request the eval builds.
class EvalPrompt {
  const EvalPrompt({
    required this.id,
    required this.request,
    this.covers = const [],
    this.desktop = '',
  });

  factory EvalPrompt.fromJson(Map<String, Object?> j) => EvalPrompt(
    id: j['id']! as String,
    request: j['request']! as String,
    covers: [for (final c in (j['covers'] as List?) ?? const []) '$c'],
    desktop: (j['desktop'] as String?) ?? '',
  );

  /// A short name, used for the build's directory.
  final String id;

  /// What the person says, word for word.
  final String request;

  /// The kit parts and capabilities the request needs.
  final List<String> covers;

  /// What degrades on the desktop harness, for the reader of the results.
  final String desktop;

  Map<String, Object?> toJson() => {
    'id': id,
    'request': request,
    'covers': covers,
    'desktop': desktop,
  };
}

/// Reads the prompt set at [path] (a JSON list of [EvalPrompt]s).
List<EvalPrompt> loadPrompts(String path) {
  final list = jsonDecode(File(path).readAsStringSync()) as List<Object?>;
  final prompts = [
    for (final p in list) EvalPrompt.fromJson(p! as Map<String, Object?>),
  ];
  final ids = <String>{};
  for (final p in prompts) {
    if (!ids.add(p.id)) throw EvalConfigError('prompt ID ${p.id} repeats');
  }
  return prompts;
}

/// A run the eval refuses: bad arguments, or one the spend guard stops.
class EvalConfigError implements Exception {
  const EvalConfigError(this.message);

  final String message;

  @override
  String toString() => message;
}

/// One eval run's settings.
class EvalConfig {
  const EvalConfig({
    required this.model,
    required this.prompts,
    required this.maxSpendUsd,
    required this.runName,
    required this.outDir,
    this.maxContinueRuns = 12,
    this.maxBuildTime = const Duration(minutes: 30),
    this.grant = true,
  });

  static const usage =
      '''
Live build eval: each prompt is built by the in-app agent (first run, then
continue runs until the plan is done, a cap, or an error).

  --model <id>          model to build with (default: the app's default)
  --prompts <a,b>       prompt IDs to run (default: all)
  --max-spend <usd>     refuse when $perBuildCapUsd x prompts exceeds it (default 30)
  --run <name>          results go to <out>/<name> (default: a timestamp)
  --out <dir>           results root (default: <repo>/target/flutter-app/eval)
  --prompt-file <path>  the prompt set (default: tool/eval/prompts.json)
  --max-runs <n>        continue runs per build before the harness stops it (default 12)
  --max-minutes <n>     wall time per build before the harness stops it (default 30)
  --no-grant            do not grant the published app its declared net hosts and AI
''';

  /// Parses [args]; [promptFile] and [outRoot] are the defaults of
  /// `--prompt-file` and `--out`, [now] names the run when `--run` is not
  /// given. Throws [EvalConfigError] on a bad argument, an unknown model or
  /// prompt ID, and when the spend guard refuses.
  factory EvalConfig.parse(
    List<String> args, {
    required String promptFile,
    required String outRoot,
    required DateTime now,
  }) {
    final opts = <String, String>{};
    var grant = true;
    for (var i = 0; i < args.length; i++) {
      final a = args[i];
      if (a == '--no-grant') {
        grant = false;
        continue;
      }
      const valued = {
        '--model',
        '--prompts',
        '--max-spend',
        '--run',
        '--out',
        '--prompt-file',
        '--max-runs',
        '--max-minutes',
      };
      if (!valued.contains(a)) throw EvalConfigError('unknown argument $a');
      if (i + 1 >= args.length) throw EvalConfigError('$a needs a value');
      opts[a] = args[++i];
    }
    final model = opts['--model'] ?? modelChoices.first.id;
    if (!modelPrices.containsKey(model)) {
      throw EvalConfigError(
        'unknown model $model; known: ${modelPrices.keys.join(', ')}',
      );
    }
    final all = loadPrompts(opts['--prompt-file'] ?? promptFile);
    final List<EvalPrompt> chosen;
    if (opts['--prompts'] case final ids?) {
      final want = [
        for (final s in ids.split(','))
          if (s.trim().isNotEmpty) s.trim(),
      ];
      chosen = [
        for (final id in want)
          all.where((p) => p.id == id).firstOrNull ??
              (throw EvalConfigError(
                'unknown prompt $id; known: ${all.map((p) => p.id).join(', ')}',
              )),
      ];
    } else {
      chosen = all;
    }
    if (chosen.isEmpty) throw const EvalConfigError('no prompts to run');
    double number(String flag, double fallback) {
      final v = opts[flag];
      if (v == null) return fallback;
      final n = double.tryParse(v);
      if (n == null || n <= 0) {
        throw EvalConfigError('$flag needs a positive number, not $v');
      }
      return n;
    }

    final maxSpend = number('--max-spend', 30);
    final runName = opts['--run'] ?? _stamp(now);
    if (runName.contains('/') || runName.startsWith('.')) {
      throw EvalConfigError('--run must be a plain name, not $runName');
    }
    final config = EvalConfig(
      model: model,
      prompts: chosen,
      maxSpendUsd: maxSpend,
      runName: runName,
      outDir: '${opts['--out'] ?? outRoot}/$runName',
      maxContinueRuns: number('--max-runs', 12).ceil(),
      maxBuildTime: Duration(
        seconds: (number('--max-minutes', 30) * 60).round(),
      ),
      grant: grant,
    );
    config.checkSpend();
    return config;
  }

  final String model;
  final List<EvalPrompt> prompts;
  final double maxSpendUsd;
  final String runName;

  /// Where this run's results go: `<out>/<runName>`.
  final String outDir;

  /// Continue runs per build before the harness stops the build.
  final int maxContinueRuns;

  /// Wall time per build before the harness stops the build.
  final Duration maxBuildTime;

  /// Whether the published app is granted its declared net hosts and AI
  /// before its final screenshots.
  final bool grant;

  /// The most the run can spend on building: [perBuildCapUsd] per prompt.
  double get worstCaseUsd => perBuildCapUsd * prompts.length;

  /// The spend guard: throws [EvalConfigError] when [worstCaseUsd] is over
  /// [maxSpendUsd].
  void checkSpend() {
    if (worstCaseUsd > maxSpendUsd) {
      throw EvalConfigError(
        'refusing to start: ${prompts.length} builds at up to '
        '\$${perBuildCapUsd.toStringAsFixed(2)} each could spend '
        '\$${worstCaseUsd.toStringAsFixed(2)}, over --max-spend '
        '\$${maxSpendUsd.toStringAsFixed(2)}',
      );
    }
  }

  /// What the run will do, one line per fact, for the console and
  /// `config.json`.
  Map<String, Object?> toJson() => {
    'model': model,
    'prompts': [for (final p in prompts) p.id],
    'max_spend_usd': maxSpendUsd,
    'per_build_cap_usd': perBuildCapUsd,
    'worst_case_usd': worstCaseUsd,
    'max_continue_runs': maxContinueRuns,
    'max_build_minutes': maxBuildTime.inSeconds / 60,
    'grant': grant,
    'out': outDir,
  };

  static String _stamp(DateTime t) {
    String two(int n) => n.toString().padLeft(2, '0');
    return '${t.year}${two(t.month)}${two(t.day)}-'
        '${two(t.hour)}${two(t.minute)}${two(t.second)}';
  }
}
