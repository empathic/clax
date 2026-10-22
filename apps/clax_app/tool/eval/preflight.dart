// Checks a live build eval's arguments and its spend guard without
// building anything, and prints what the run will do:
//
//   dart run tool/eval/preflight.dart <args>     (from apps/clax_app)
//
// Exits 0 when the run may start, 2 when it is refused.
import 'dart:convert';
import 'dart:io';

import 'eval_config.dart';

void main(List<String> args) {
  final app = File(Platform.script.toFilePath()).parent.parent.parent.path;
  try {
    final config = EvalConfig.parse(
      args,
      promptFile: '$app/tool/eval/prompts.json',
      outRoot:
          '${Directory('$app/../..').resolveSymbolicLinksSync()}'
          '/target/flutter-app/eval',
      now: DateTime.now(),
    );
    stdout.writeln(const JsonEncoder.withIndent('  ').convert(config));
  } on EvalConfigError catch (e) {
    stderr.writeln('eval: $e\n\n${EvalConfig.usage}');
    exit(2);
  }
}
