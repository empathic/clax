import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter_test/flutter_test.dart';

/// When the dot crosses the middle, moving right (true) or left, from 0 to
/// [until] ms in 4 ms steps.
List<(double, bool)> _crossings(ClaxSynapseModel m, double until) {
  final out = <(double, bool)>[];
  var last = ClaxSynapseModel.centreX;
  for (var t = 0.0; t <= until; t += 4) {
    m.seek(t);
    final mid = ClaxSynapseModel.centreX;
    if (last != mid &&
        m.dotX != mid &&
        (last - mid).sign != (m.dotX - mid).sign) {
      out.add((t, m.dotX > mid));
    }
    last = m.dotX;
  }
  return out;
}

void main() {
  test('it begins as the mark and opens into a synapse', () {
    final m = ClaxSynapseModel(seed: 1)..seek(0);
    expect(m.open, 0);
    expect(m.dotX, ClaxSynapseModel.centreX);
    expect(m.dotY, ClaxMarkGeometry.dot.dy);
    m.seek(ClaxSynapseModel.openMs);
    expect(m.open, 1);
  });

  test('the dot fires to and fro, each side in turn', () {
    final m = ClaxSynapseModel(seed: 1);
    final crossings = _crossings(m, 8000);
    expect(crossings.length, greaterThan(4));
    for (final (i, (_, right)) in crossings.indexed) {
      expect(right, i.isEven, reason: 'crossing $i');
    }
  });

  test('the rhythm is not metronomic', () {
    for (var seed = 0; seed < 5; seed++) {
      final m = ClaxSynapseModel(seed: seed);
      final at = [for (final (t, _) in _crossings(m, 10000)) t];
      final gaps = {
        for (var i = 1; i < at.length; i++) (at[i] - at[i - 1]).round() ~/ 40,
      };
      expect(gaps.length, greaterThan(2), reason: 'seed $seed: $at');
    }
  });

  test('a firing looses ions, and its path leaves the straight line', () {
    final m = ClaxSynapseModel(seed: 2);
    var ions = 0;
    var off = 0.0;
    for (var t = 0.0; t < 3000; t += 8) {
      m.seek(t);
      ions = ions > m.ionCount ? ions : m.ionCount;
      final d = (m.dotY - ClaxMarkGeometry.dot.dy).abs();
      if (d > off) off = d;
    }
    expect(ions, greaterThanOrEqualTo(3));
    expect(off, greaterThan(0.2));
  });

  test('the finish lands, sparkles, and ends as the resting mark', () {
    final m = ClaxSynapseModel(seed: 3);
    for (var t = 0.0; t <= 1234; t += 16) {
      m.seek(t);
    }
    m.finishAt(1234);
    m.seek(1234 + ClaxSynapseModel.landMs + 60);
    expect(m.sparkle, isNotNull);
    expect(m.agentScale, greaterThan(1.05), reason: 'Clax’s arc nods');
    m.seek(1234 + ClaxSynapseModel.finishMs - 1);
    expect(m.finished, isFalse);
    m.seek(1234 + ClaxSynapseModel.finishMs);
    expect(m.finished, isTrue);
    expect(m.open, 0);
    expect(m.dotX, ClaxSynapseModel.centreX);
    expect(m.dotY, ClaxMarkGeometry.dot.dy);
    expect(m.dotScale, 1);
    expect(m.youScale, 1);
    expect(m.agentScale, 1);
    expect(m.ionCount, 0);
    expect(m.sparkle, isNull);
    expect(ClaxSynapse.finishLength.inMilliseconds, ClaxSynapseModel.finishMs);
  });
}
