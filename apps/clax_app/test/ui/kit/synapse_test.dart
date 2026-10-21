import 'package:clax_app/ui/kit/kit.dart';
import 'package:flutter_test/flutter_test.dart';

typedef _Dot = ({double x, double alpha});

List<_Dot> _dots(ClaxSynapseModel m) => [
  for (var i = 0; i < m.dotCount; i++)
    (x: m.dots[2 * i], alpha: m.dots[2 * i + 1]),
];

/// When each dot leaves, from 0 to [until] ms in 4 ms steps: a new dot is
/// one more in flight than the step before, or a first one near the arc.
List<double> _departures(ClaxSynapseModel m, double until) {
  final out = <double>[];
  var before = <_Dot>[];
  for (var t = 0.0; t <= until; t += 4) {
    m.seek(t);
    final now = _dots(m);
    if (now.isNotEmpty && (before.isEmpty || now.last.x < before.last.x)) {
      out.add(t);
    }
    before = now;
  }
  return out;
}

void main() {
  test(
    'it begins as the resting mark, and its middle dot fades as the dots start',
    () {
      final m = ClaxSynapseModel(seed: 1)..seek(0);
      expect(m.middle, 1);
      expect(m.dotCount, 0);
      m.seek(ClaxSynapseModel.middleFade);
      expect(m.middle, 0);
      expect(m.dotCount, greaterThan(0));
    },
  );

  test('each dot goes from the person’s arc to Clax’s, fading as it goes', () {
    final m = ClaxSynapseModel(seed: 1);
    m.seek(ClaxSynapseModel.firstDot + 200);
    final early = _dots(m).first;
    m.seek(ClaxSynapseModel.firstDot + 700);
    final late = _dots(m).first;
    expect(early.x, greaterThan(ClaxSynapseModel.fromX));
    expect(late.x, greaterThan(early.x));
    expect(late.x, lessThanOrEqualTo(ClaxSynapseModel.toX));
    expect(late.alpha, lessThan(early.alpha));
    final all = ClaxSynapseModel(seed: 1);
    for (var t = 0.0; t < 5000; t += 8) {
      all.seek(t);
      for (final d in _dots(all)) {
        expect(d.alpha, inInclusiveRange(0, 1));
        expect(
          d.x,
          inInclusiveRange(ClaxSynapseModel.fromX, ClaxSynapseModel.toX),
        );
      }
    }
  });

  test('the dots come in a steady stream, a little uneven', () {
    for (var seed = 0; seed < 5; seed++) {
      final at = _departures(ClaxSynapseModel(seed: seed), 8000);
      final gaps = [for (var i = 1; i < at.length; i++) at[i] - at[i - 1]];
      expect(gaps.length, greaterThan(10));
      for (final g in gaps) {
        expect(
          g,
          inInclusiveRange(
            ClaxSynapseModel.spacing - ClaxSynapseModel.spacingJitter - 8,
            ClaxSynapseModel.spacing + ClaxSynapseModel.spacingJitter + 8,
          ),
          reason: 'seed $seed: $gaps',
        );
      }
      expect(gaps.toSet().length, greaterThan(2), reason: 'seed $seed');
    }
  });

  test('the finish stops the stream and settles as the resting mark', () {
    final m = ClaxSynapseModel(seed: 3);
    for (var t = 0.0; t <= 1234; t += 16) {
      m.seek(t);
    }
    expect(m.dotCount, greaterThan(0));
    m.finishAt(1234);
    var count = m.dotCount;
    for (var t = 1234.0; t < 1234 + ClaxSynapseModel.finishMs; t += 4) {
      m.seek(t);
      expect(m.dotCount, lessThanOrEqualTo(count), reason: 'none leaves');
      count = m.dotCount;
    }
    m.seek(1234 + ClaxSynapseModel.finishMs - 1);
    expect(m.finished, isFalse);
    m.seek(1234 + ClaxSynapseModel.finishMs);
    expect(m.finished, isTrue);
    expect(m.dotCount, 0);
    expect(m.middle, 1);
    expect(ClaxSynapse.finishLength.inMilliseconds, ClaxSynapseModel.finishMs);
  });
}
