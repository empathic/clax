import 'dart:async';

import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/shared_stream.dart';
import 'package:flutter_test/flutter_test.dart';

/// A [ClaxStream] the test drives: [fail] makes the next subscribes throw.
class _Inner implements ClaxStream {
  final out = StreamController<StreamEvent>.broadcast();
  final subscribed = <List<String>>[];
  final List<Object> fail = [];
  var closed = false;

  @override
  Stream<StreamEvent> get events => out.stream;

  @override
  Future<void> subscribe(List<String> topics) async {
    if (closed) throw StateError('the stream is closed');
    if (fail.isNotEmpty) throw fail.removeAt(0);
    subscribed.add(List.of(topics));
  }

  @override
  Future<void> close() async {
    closed = true;
    await out.close();
  }
}

const _noAnswer = ClaxApiError(
  status: 0,
  code: ClaxApiError.unreachable,
  message: 'no answer',
);

Future<void> _settle() async {
  for (var i = 0; i < 20; i++) {
    await Future<void>.delayed(Duration.zero);
  }
}

void main() {
  late List<_Inner> inners;
  late List<Duration> waits;
  late List<Completer<void>> wakes;
  late SharedClaxStream shared;
  late List<StreamEvent> seen;

  setUp(() {
    inners = [];
    waits = [];
    wakes = [];
    seen = [];
    shared = SharedClaxStream(
      () {
        final i = _Inner();
        inners.add(i);
        return i;
      },
      delay: (d) {
        waits.add(d);
        final c = Completer<void>();
        wakes.add(c);
        return c.future;
      },
    );
    shared.events.listen(seen.add);
  });

  tearDown(() => shared.close());

  Future<void> wake() async {
    wakes.removeAt(0).complete();
    await _settle();
  }

  test('screens share one connection and its events', () async {
    await shared.subscribe(['gallery', 'inbox', 'questions']);
    await shared.subscribe(['inbox']);
    final other = <StreamEvent>[];
    final sub = shared.events.listen(other.add);
    inners.single.out.add(
      const StreamEvent(topic: 'inbox', type: 'inbox_item', data: {}),
    );
    await _settle();
    expect(inners, hasLength(1));
    expect(seen.single.type, 'inbox_item');
    expect(other.single.type, 'inbox_item');
    await sub.cancel();
    expect(inners.single.closed, isFalse, reason: 'a screen leaving keeps it');
  });

  test('a dropped topic is subscribed again, then resynced; the drop is not '
      'passed on', () async {
    await shared.subscribe(['inbox', 'questions']);
    inners.single.fail.add(_noAnswer);
    inners.single.out.add(
      const StreamEvent(
        topic: 'questions',
        type: 'unsubscribed',
        data: {'topic': 'questions', 'code': 'x', 'message': 'y'},
      ),
    );
    await _settle();
    expect(seen, isEmpty);
    await wake(); // fails: no answer
    await wake();
    expect(waits, const [Duration(seconds: 1), Duration(seconds: 2)]);
    expect(inners.single.subscribed.last, ['questions']);
    expect(seen.single.type, 'resync');
    expect(seen.single.topic, 'questions');
    expect(seen.single.data['reason'], 'reconnect');
  });

  test('an ended stream is opened again with every topic', () async {
    await shared.subscribe(['gallery', 'questions']);
    inners.single.out.addError(StateError('refused'));
    await _settle();
    expect(seen, isEmpty);
    await wake();
    expect(inners, hasLength(2));
    expect(inners.first.closed, isTrue);
    expect(
      inners.last.subscribed.single,
      unorderedEquals(['gallery', 'questions']),
    );
    expect([
      for (final e in seen) e.topic,
    ], unorderedEquals(['gallery', 'questions']));
    expect(seen.every((e) => e.type == 'resync'), isTrue);
  });

  test(
    'a subscribe that got no answer throws, and keeps trying behind',
    () async {
      _Inner? first;
      shared.events; // opens the first inner
      first = inners.single;
      first.fail.add(_noAnswer);
      await expectLater(shared.subscribe(['questions']), throwsA(_noAnswer));
      expect(shared.topics, {'questions'});
      await wake();
      expect(first.subscribed.single, ['questions']);
      expect(seen.single.type, 'resync');
    },
  );

  test('close ends the connection and the events', () async {
    await shared.subscribe(['inbox']);
    var done = false;
    shared.events.listen(null, onDone: () => done = true);
    await shared.close();
    await _settle();
    expect(inners.single.closed, isTrue);
    expect(done, isTrue);
    expect(() => shared.subscribe(['inbox']), throwsStateError);
  });
}
