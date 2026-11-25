// lib/timing_lint.dart's rules, each on a snippet that breaks it and one
// that does not, and the exemption comments. A plain program: it prints
// each failed case and exits 1 if any failed.
import 'dart:io';

import 'package:clax_timing_lint/timing_lint.dart';

var _failed = 0;

/// The rules [source] breaks, as `line:rule`, in a file at [path].
List<String> _found(String source, {String path = 'app/test/a_test.dart'}) => [
  for (final f in lint(path, source, stuckAfter: const Duration(minutes: 2)))
    '${f.line}:${f.rule}',
];

void _expect(String what, String source, List<String> want, {String? path}) {
  final got = path == null ? _found(source) : _found(source, path: path);
  if (got.join(' ') != want.join(' ')) {
    _failed++;
    stderr.writeln('FAIL $what\n  want $want\n  got  $got');
  }
}

const _integration = 'app/integration_test/a_test.dart';

void main() {
  _expect('sleep', 'void f() { sleep(const Duration(seconds: 1)); }', [
    '1:timing_sleep',
  ]);
  _expect('a method named sleep', 'void f(x) { x.sleep(1); }', []);

  _expect(
    'a delay inside runAsync',
    'f(t) async { await t.runAsync(() => Future<void>.delayed(const Duration(milliseconds: 10))); }',
    ['1:timing_delay'],
  );
  _expect(
    'a zero delay inside runAsync',
    'f(t) async { await t.runAsync(() => Future<void>.delayed(Duration.zero)); }',
    [],
  );
  _expect(
    'a delay in fake time',
    'f() async { await Future.delayed(const Duration(seconds: 1)); }',
    [],
  );
  _expect(
    'a delay in an integration test',
    'f() async { await Future.delayed(Duration(milliseconds: 50)); }',
    ['1:timing_delay'],
    path: _integration,
  );
  _expect(
    'a clock delays and reads real time',
    'class C implements Clock { DateTime now() => DateTime.now(); '
        'Future<void> delay(Duration d) => Future<void>.delayed(d); }',
    [],
    path: _integration,
  );

  _expect(
    'a pump in fake time',
    'f(t) async { await t.pump(const Duration(seconds: 1)); }',
    [],
  );
  _expect(
    'a pump in an integration test',
    'f(t) async { await t.pump(const Duration(milliseconds: 500)); await t.pump(); }',
    ['1:timing_pump'],
    path: _integration,
  );

  _expect('DateTime.now', 'f() => DateTime.now();', ['1:timing_wall_clock']);
  _expect('a Stopwatch', 'f() { final w = Stopwatch()..start(); }', [
    '1:timing_wall_clock',
  ]);
  _expect('a const Stopwatch', 'f() { final w = new Stopwatch(); }', [
    '1:timing_wall_clock',
  ]);
  _expect('DateTime.utc', 'f() => DateTime.utc(2026);', []);

  _expect(
    'a short timeout',
    'f(x) => x.timeout(const Duration(seconds: 10));',
    ['1:timing_short_timeout'],
  );
  _expect('a timeout at stuckAfter', 'f(x) => x.timeout(stuckAfter);', []);
  _expect(
    'a long timeout',
    'f(x) => x.timeout(const Duration(minutes: 3));',
    [],
  );
  _expect(
    'a short timeout by a named constant',
    'const _cap = Duration(seconds: 60);\nf(x) => x.timeout(_cap);',
    ['2:timing_short_timeout'],
  );
  _expect(
    'a short test timeout',
    "f() => testWidgets('x', (t) async {}, timeout: const Timeout(Duration(seconds: 30)));",
    ['1:timing_short_timeout'],
  );
  _expect(
    'a hang guard test timeout',
    "f() => testWidgets('x', (t) async {}, timeout: const Timeout(Duration(minutes: 5)));",
    [],
  );

  _expect(
    'counted zero delays',
    'f() async {\n  for (var i = 0; i < 20; i++) {\n    await Future<void>.delayed(Duration.zero);\n  }\n}',
    ['2:timing_turn_count'],
  );
  _expect(
    'counted runAsync turns',
    'f(t) async { for (var i = 0; i < 6; i++) { await t.runAsync(() async {}); await t.pump(); } }',
    ['1:timing_turn_count'],
  );
  _expect(
    'counted fake frames',
    'f(t) async { for (var i = 0; i < 8; i++) { await t.pump(const Duration(milliseconds: 50)); } }',
    [],
  );
  _expect(
    'counted key presses, a pump after each',
    'f(t) async { for (var i = 0; i < 4; i++) { await t.sendKeyEvent(k); await t.pump(); } }',
    [],
  );
  _expect(
    'counted single-statement turns',
    'f(t) async { for (var i = 0; i < 5; i++) await t.pump(); }',
    ['1:timing_turn_count'],
  );
  _expect(
    'a turn loop bounded by a condition',
    'f(t, n) async { for (var i = 0; i < n; i++) { await pumpEventQueue(); } }',
    [],
  );
  _expect('pumpEventQueue with a count', 'f() => pumpEventQueue(times: 50);', [
    '1:timing_turn_count',
  ]);
  _expect('pumpEventQueue', 'f() => pumpEventQueue();', []);

  _expect(
    'an exemption on the line',
    'f() => DateTime.now(); // ignore: timing_wall_clock -- the hang guard',
    [],
  );
  _expect(
    'an exemption on the line before',
    '// ignore: timing_wall_clock -- the hang guard\nf() => DateTime.now();',
    [],
  );
  _expect(
    'an exemption among other ignores',
    '// ignore: unused_element, timing_wall_clock -- the hang guard\nf() => DateTime.now();',
    [],
  );
  _expect(
    'an exemption of another rule',
    '// ignore: timing_sleep -- wrong rule\nf() => DateTime.now();',
    ['1:timing_stale_ignore', '2:timing_wall_clock'],
  );
  _expect(
    'an exemption without a reason',
    '// ignore: timing_wall_clock\nf() => DateTime.now();',
    ['1:timing_ignore_reason', '2:timing_wall_clock'],
  );
  _expect(
    'a stale exemption',
    'f() => 1; // ignore: timing_wall_clock -- nothing here',
    ['1:timing_stale_ignore'],
  );
  _expect(
    'other ignores are not read',
    '// ignore: unused_element\nf() => 1;',
    [],
  );

  final settle = stuckAfterIn('const stuckAfter = Duration(minutes: 2);');
  if (settle != const Duration(minutes: 2)) {
    _failed++;
    stderr.writeln('FAIL stuckAfterIn: $settle');
  }

  if (_failed > 0) {
    stderr.writeln('$_failed failed');
    exit(1);
  }
  print('all passed');
}
