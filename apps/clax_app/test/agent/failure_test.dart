import 'package:clax_app/agent/failure.dart';
import 'package:clax_app/ui/kit/verbs.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  const progress = LimitProgress(
    stops: 2,
    spentUsd: 2.1,
    notes: [
      'Adding the chart canvas to the stats screen.',
      'Wiring the run-type filter to the chart.',
    ],
    changed: [
      (path: 'index.html', added: 42, removed: 10),
      (path: 'app.js', added: 8, removed: 0),
      (path: 'notes.txt', added: 0, removed: 0),
    ],
  );

  group('parseFailureReply', () {
    test('reads back the cost, cap, files and notes a limit stop was built '
        'from', () {
      final r = parseFailureReply(
        limitFailureReply('10 turns', leftUsd: 6 - 2.1, progress: progress),
      );
      expect(
        r.line,
        'It was too big for one go — it needed more than 10 steps. '
        '$keepGoingHint',
      );
      expect(r.stops, 2);
      expect(r.costUsd, 2.1);
      expect(r.capUsd, 6);
      expect(r.files, [
        (path: 'index.html', added: 42, removed: 10),
        (path: 'app.js', added: 8, removed: 0),
        (path: 'notes.txt', added: 0, removed: 0),
      ]);
      expect(r.notes, progress.notes);
      expect(r.raw, isNull);
      expect(r.spentLimit, isFalse);
    });

    test('counts the files a long list leaves out', () {
      final r = parseFailureReply(
        limitFailureReply(
          '10 turns',
          leftUsd: 5,
          progress: LimitProgress(
            stops: 1,
            spentUsd: 1,
            changed: [
              for (var i = 0; i < 7; i++) (path: 'f$i.js', added: 1, removed: 0),
            ],
          ),
        ),
      );
      expect(r.files, hasLength(maxReplyFiles));
      expect(r.moreFiles, 2);
    });

    test('a run that started over says so first among the notes', () {
      final r = parseFailureReply(
        limitFailureReply(
          '10 turns',
          leftUsd: 5,
          progress: const LimitProgress(
            stops: 1,
            spentUsd: 1,
            startedOver: true,
            notes: ['Rebuilt the list.'],
          ),
        ),
      );
      expect(r.notes, [startedOverLine, 'Rebuilt the list.']);
    });

    test('a change that spent its limit has spent it', () {
      expect(
        parseFailureReply(
          limitFailureReply(
            '10 turns',
            leftUsd: 0,
            progress: const LimitProgress(stops: 4, spentUsd: 6),
          ),
        ).spentLimit,
        isTrue,
      );
      // Under minKeepGoingUsd left is spent too: the reply says it cannot
      // keep going.
      expect(
        parseFailureReply(
          limitFailureReply(
            '10 turns',
            leftUsd: 0.02,
            progress: const LimitProgress(stops: 4, spentUsd: 5.98),
          ),
        ).spentLimit,
        isTrue,
      );
      // A reply with no progress says so in words.
      expect(
        parseFailureReply(limitFailureReply('10 turns', leftUsd: 0)).spentLimit,
        isTrue,
      );
    });

    test('a plain failure is its reason as a sentence, the agent\'s words as '
        'a note', () {
      final r = parseFailureReply(
        limitFailureReply('10 turns', leftUsd: 6, agentSaid: 'Ran out of room'),
      );
      expect(r.notes, ['Ran out of room']);
      final plain = parseFailureReply(
        "Couldn't make that change: this app may only reach "
        "api.weather.example, so it can't reach strava.com.",
      );
      expect(
        plain.line,
        "This app may only reach api.weather.example, so it can't reach "
        'strava.com.',
      );
      expect(plain.notes, isEmpty);
      expect(plain.files, isEmpty);
      expect(plain.costUsd, isNull);
      expect(plain.raw, isNull);
      expect(plain.spentLimit, isFalse);
    });

    test('a raw error reads as one plain line, the raw text kept', () {
      final r = parseFailureReply(
        'Couldn’t make that change: The Claude API said: overloaded (529)',
      );
      expect(r.line, 'Claude is busy right now. Try again in a minute.');
      expect(r.raw, 'The Claude API said: overloaded (529)');
    });

    test('a planned stop keeps its sentence and its progress', () {
      final r = parseFailureReply(
        plannedStopReply('10 turns', progress: progress),
      );
      expect(r.line, plannedStopReason('10 turns'));
      expect(r.costUsd, 2.1);
      expect(r.notes, progress.notes);
    });
  });

  group('Keep going', () {
    test('the hint names the pointer\'s verb', () {
      expect(
        keepGoingHintFor(ClaxVerbs.touch),
        'Tap Keep going to pick up where it stopped.',
      );
      expect(
        keepGoingHintFor(ClaxVerbs.mouse),
        'Click Keep going to pick up where it stopped.',
      );
      // Replies are stored in touch's words.
      expect(keepGoingHint, keepGoingHintFor(ClaxVerbs.touch));
    });

    test('is offered on the stored sentence and on the new one', () {
      expect(
        offersKeepGoing('x. Tap Keep going to pick up where it stopped.'),
        isTrue,
      );
      expect(offersKeepGoing(keepGoingHintFor(ClaxVerbs.mouse)), isTrue);
      expect(offersKeepGoing(keepGoingHintFor(ClaxVerbs.touch)), isTrue);
      expect(offersKeepGoing('Keep going to pick up where it stopped.'), isFalse);
      expect(offersKeepGoing(keepGoingLine(0.01)), isFalse);
    });

    test('the sentence is shown in the reader\'s words', () {
      final stored = keepGoingLine(1.2);
      expect(
        keepGoingIn(stored, ClaxVerbs.mouse),
        'Click Keep going to pick up where it stopped. About \$1.20 of its '
        '\$6 is left.',
      );
      expect(
        keepGoingIn(keepGoingIn(stored, ClaxVerbs.mouse), ClaxVerbs.touch),
        stored,
      );
    });
  });
}
