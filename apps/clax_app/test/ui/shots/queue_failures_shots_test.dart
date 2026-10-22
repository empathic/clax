// Makes the queue could not finish, rendered on the phone, light and dark,
// into CLAX_SHOTS_DIR (skipped without it): the launcher's tiles of a make
// whose turn failed, one whose job left the queue and one whose builder
// stopped (`make-failed-tiles`), and the build view of each: a failure in
// words with Developer tools' Details open on its code
// (`make-failed-words`), a make whose job left the queue
// (`make-left-queue`), and a stopped builder, which offers no Retry
// (`make-builder-stopped`).
//
//   SHOTS="$PWD/target/flutter-app/shots"; mkdir -p "$SHOTS"
//   bazelisk test //apps/clax_app:ui_shots_queue_failures_shots_test \
//     --nocache_test_results --test_env=CLAX_SHOTS_DIR="$SHOTS" \
//     --sandbox_writable_path="$SHOTS"
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/queue/client.dart';
import 'package:clax_app/queue/isolate_client.dart' show builderStoppedError;
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:clax_app/ui/making_screen.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../../support/fake_queue.dart';
import '../../support/fake_voice.dart';
import 'rig.dart';

/// A queue holding three makes it could not finish, and the service over
/// it; the job of `left` left the queue before it ended.
(FakeQueueClient, MakeService) _failures() {
  final q = FakeQueueClient()
    ..push([
      makeJob('left', state: JobState.queued, title: 'A packing list for trips'),
      makeJob(
        'turn',
        state: JobState.failed,
        title: 'A tide table for the harbour',
        error: 'turn_failed',
      ),
      makeJob(
        'halted',
        state: JobState.failed,
        title: 'A water reminder every two hours',
        error: builderStoppedError,
        offers: const {},
      ),
    ]);
  final makes = MakeService(q);
  q.push([
    for (final j in q.state.value.jobs)
      if (j.id != 'left') j,
  ]);
  addTearDown(makes.dispose);
  return (q, makes);
}

/// The build view of make [id] of [makes], Developer tools on when
/// [developer].
Future<void> _build(
  Shot s,
  MakeService makes,
  String id, {
  bool developer = false,
}) async {
  final on = ValueNotifier(developer);
  addTearDown(on.dispose);
  final session = makes.sessionOf(id)!;
  await s.tester.pumpWidget(
    DeveloperScope(
      notifier: on,
      child: s.app(
        MakingScreen(request: session.request, session: session),
        title: 'New app',
      ),
    ),
  );
  await s.settle();
}

void main() {
  shots('make-failed-tiles', (s) async {
    final (_, makes) = _failures();
    final api = FakeClaxApi();
    api.seedArtifact('Notes', {'index.html': '<p>n</p>'});
    await s.tester.pumpWidget(
      s.app(
        Launcher(
          api: api,
          open: (_) {},
          make: (r, cancelled, stage) => throw StateError('unused'),
          voice: FakeVoice.unsupported('No speech here'),
          makes: makes,
        ),
      ),
    );
    await s.settle(10);
    expect(find.text('Builder stopped'), findsOneWidget);
    expect(find.text('Couldn’t make it'), findsNWidgets(2));
    await s.take('make-failed-tiles');
  }, forms: [phone]);

  shots('make-failed-words', (s) async {
    final (_, makes) = _failures();
    await _build(s, makes, 'turn', developer: true);
    await s.tester.tap(find.byKey(const ValueKey('make-failure-details')));
    await s.settle();
    expect(find.text('Claude couldn’t finish this step. Try again.'), findsOneWidget);
    expect(find.text('turn_failed'), findsOneWidget);
    await s.take('make-failed-words');
  }, forms: [phone]);

  shots('make-left-queue', (s) async {
    final (_, makes) = _failures();
    await _build(s, makes, 'left');
    expect(find.text('Clax couldn’t start this make. Try again.'), findsOneWidget);
    expect(find.text('Retry'), findsOneWidget);
    await s.take('make-left-queue');
  }, forms: [phone]);

  shots('make-builder-stopped', (s) async {
    final (_, makes) = _failures();
    await _build(s, makes, 'halted');
    expect(
      find.text('Clax’s builder stopped. Restart Clax to go on.'),
      findsOneWidget,
    );
    expect(find.text('Retry'), findsNothing);
    await s.take('make-builder-stopped');
  }, forms: [phone]);
}
