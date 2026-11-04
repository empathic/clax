// The timing rules (lib/timing_lint.dart) over the app's tests:
//
//   dart bin/test_timing.dart <app directory>
//
// prints one `path:line: rule: hint` per finding in <app>/test and
// <app>/integration_test, and exits 1 when there is any.
import 'dart:io';

import 'package:clax_timing_lint/timing_lint.dart';

void main(List<String> args) {
  if (args.length != 1) {
    stderr.writeln('usage: test_timing.dart <app directory>');
    exit(64);
  }
  final app = args.single;
  final settle = File('$app/test/support/settle.dart');
  final stuckAfter =
      (settle.existsSync() ? stuckAfterIn(settle.readAsStringSync()) : null) ??
      const Duration(minutes: 2);
  final files = [
    for (final dir in ['test', 'integration_test'])
      if (Directory('$app/$dir').existsSync())
        ...Directory('$app/$dir')
            .listSync(recursive: true)
            .whereType<File>()
            .where((f) => f.path.endsWith('.dart')),
  ]..sort((a, b) => a.path.compareTo(b.path));
  final found = [
    for (final f in files)
      ...lint(f.path, f.readAsStringSync(), stuckAfter: stuckAfter),
  ];
  found.forEach(print);
  if (found.isEmpty) {
    print('test timing: no findings in ${files.length} files');
    return;
  }
  print('test timing: ${found.length} findings in ${files.length} files');
  exit(1);
}
