import 'package:clax_app/queue/client.dart';
import 'package:clax_app/ui/local_queue.dart' show interruptedError;
import 'package:clax_app/ui/make_service.dart';
import 'package:flutter_test/flutter_test.dart' hide Retry;

import '../support/fake_queue.dart';

void main() {
  test('a make is a Make intent, and is listed as the queue shows it', () {
    final q = FakeQueueClient();
    final makes = MakeService(q);
    final s = makes.start('a timer');
    final make = q.sentOf<Make>().single;
    expect(make.id, s.id);
    expect(make.request, 'a timer');
    expect(makes.makes, isEmpty, reason: 'not listed before the queue has it');
    q.push([makeJob(s.id, state: JobState.queued)]);
    expect(makes.makes, [s]);
    expect(s.queued, isTrue);
    q.push([makeJob(s.id)]);
    expect(s.running, isTrue);
    expect(s.queued, isFalse);
  });

  test('jobs the UI did not start are listed newest first, with their '
      'titles', () {
    final q = FakeQueueClient()
      ..push([
        makeJob('a', title: 'runs'),
        makeJob('b', title: 'habits', state: JobState.queued),
        changeJob('c', 'app-1'),
      ]);
    final makes = MakeService(q);
    expect([for (final m in makes.makes) m.request], ['habits', 'runs']);
  });

  test('where a make stands follows its job', () {
    final q = FakeQueueClient()..push([makeJob('j')]);
    final s = MakeService(q).sessionOf('j')!;
    var heard = 0;
    s.addListener(() => heard++);

    q.put(makeJob('j', waitingFor: WaitingFor.claude));
    expect(s.waitingForConnection, isTrue);
    expect(heard, 1);

    q.put(makeJob('j', state: JobState.stopped));
    expect(s.isStopped, isTrue);
    expect(s.running, isFalse);
    expect(s.canKeepGoing, isTrue);
    expect(s.stopped, isNotNull);

    q.put(makeJob('j', state: JobState.failed, error: 'doc_moved'));
    expect(s.failure, contains('A document changed'));

    q.put(makeJob('j', state: JobState.failed, error: interruptedError));
    expect(s.interrupted, isTrue);
    expect(s.failure, isNull);

    q.put(makeJob('j', state: JobState.done, artifactId: 'app-9'));
    expect(s.aid, 'app-9');
  });

  test('the person\'s choices are intents, sent only when offered', () {
    final q = FakeQueueClient()..push([makeJob('j')]);
    final makes = MakeService(q);
    final s = makes.sessionOf('j')!;
    s.keepGoing();
    s.retry();
    expect(q.sent, isEmpty, reason: 'a running make offers neither');

    q.put(makeJob('j', state: JobState.stopped));
    s.keepGoing();
    expect(q.sent.last, const KeepGoing('j'));

    q.put(makeJob('j', state: JobState.failed));
    s.retry();
    expect(q.sent.last, const Retry('j'));

    makes.resume(s);
    expect(q.sent.last, const Retry('j'));
  });

  test('Cancel cancels a make going on, and discards one that stopped or '
      'failed; either leaves the list at once', () {
    final q = FakeQueueClient()
      ..push([makeJob('a'), makeJob('b', state: JobState.failed)]);
    final makes = MakeService(q);
    final a = makes.sessionOf('a')!;
    final b = makes.sessionOf('b')!;
    a.cancel();
    makes.dismiss(b);
    expect(q.sent, [const Cancel('a'), const Discard('b')]);
    expect(makes.makes, isEmpty);
    expect(a.cancelled, isTrue);
  });

  test('Start over of a stopped make discards its job and makes a new one, '
      'the same session following it', () {
    final q = FakeQueueClient()..push([makeJob('j', state: JobState.stopped)]);
    final makes = MakeService(q);
    final s = makes.sessionOf('j')!;
    s.retry();
    expect(q.sent.first, const Discard('j'));
    final again = q.sent.last as Make;
    expect(again.request, 'a timer');
    expect(s.id, again.id);
    expect(s.queued, isTrue);
    expect(makes.sessionOf(again.id), same(s));
  });

  test('a make that ends while no build view shows it is told; one shown '
      'is not', () {
    final q = FakeQueueClient()
      ..push([makeJob('a'), makeJob('b'), makeJob('c'), makeJob('d')]);
    final made = <String>[];
    final failed = <String>[];
    final stopped = <String>[];
    final makes = MakeService(
      q,
      onMade: (s, aid) => made.add('${s.id}:$aid'),
      onFailed: (s) => failed.add(s.id),
      onStopped: (s) => stopped.add(s.id),
    );
    makes.sessionOf('d')!.attach();
    q.push([
      makeJob('a', state: JobState.done, artifactId: 'app-a'),
      makeJob('b', state: JobState.failed),
      makeJob('c', state: JobState.stopped),
      makeJob('d', state: JobState.done, artifactId: 'app-d'),
    ]);
    expect(made, ['a:app-a']);
    expect(failed, ['b']);
    expect(stopped, ['c']);
    expect([for (final m in makes.makes) m.id], ['c', 'b']);
  });

  test('two makes of the same words are two jobs', () {
    final q = FakeQueueClient();
    final makes = MakeService(q);
    final a = makes.start('a timer');
    final b = makes.start('a timer');
    expect(a.id, isNot(b.id));
    expect(a.id, hasLength(24));
  });
}
