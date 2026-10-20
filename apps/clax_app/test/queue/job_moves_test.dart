import 'dart:convert';
import 'dart:io';

import 'package:clax_app/queue/job_moves.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/paths.dart';

/// The store's table of job moves (`clax_store::jobs::may_move`), kept in
/// crates/clax-store/tests/fixtures/job-moves.json and checked against the
/// store by its own test (src/job_moves_fixture.rs): the queue's
/// [mayMove] must answer each row the same, so the queue never plans a
/// move the store refuses.
void main() {
  final file =
      jsonDecode(
            File(
              appPath('../../crates/clax-store/tests/fixtures/job-moves.json'),
            ).readAsStringSync(),
          )
          as Map<String, dynamic>;

  test('the states are the store\'s, in its order', () {
    expect(file['states'], JobState.values.map((s) => s.name).toList());
  });

  JobMove moveOf(String by) => switch (by) {
    'ask' => const MoveAsk(),
    'close' => const MoveClose(),
    'step' => const MoveStep(done: false),
    'step:done' => const MoveStep(done: true),
    _ when by.startsWith('to:') => MoveTo(JobState.parse(by.substring(3))!),
    _ => throw ArgumentError(by),
  };

  final rows = (file['moves'] as List).cast<Map<String, dynamic>>();

  test('every state, move and wait is in the table', () {
    final moves = {
      for (final s in JobState.values) 'to:${s.name}',
      'ask',
      'close',
      'step',
      'step:done',
    };
    expect(
      rows.length,
      JobState.values.length * moves.length * 2,
    );
    expect(rows.map((r) => r['by']).toSet(), moves);
  });

  for (final r in rows) {
    final from = JobState.parse(r['from'] as String)!;
    final by = moveOf(r['by'] as String);
    final waits = r['waits'] as bool;
    final want = r['to'] == null ? null : JobState.parse(r['to'] as String);
    test('${from.name} by $by${waits ? ' waiting' : ''} → ${want?.name}', () {
      expect(mayMove(from, by, waits: waits), want);
    });
  }
}
