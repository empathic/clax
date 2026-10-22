import 'package:clax_app/ui/overlay_gate.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('questions wait while the composer shows, and are granted on its '
      'release', () async {
    final gate = OverlayGate();
    addTearDown(gate.dispose);
    final freed = <OverlayKind>[];
    gate.freed.listen(freed.add);

    expect(gate.request(OverlayKind.composer), isTrue);
    expect(gate.showing, OverlayKind.composer);
    expect(gate.request(OverlayKind.questions), isFalse);
    expect(gate.showing, OverlayKind.composer);

    gate.release(OverlayKind.composer);
    await pumpEventQueue();
    expect(freed, [OverlayKind.questions]);
    expect(gate.showing, OverlayKind.questions);
  });

  test('questions wait while a prompt is up', () async {
    final gate = OverlayGate();
    addTearDown(gate.dispose);
    final freed = <OverlayKind>[];
    gate.freed.listen(freed.add);

    expect(gate.request(OverlayKind.ask), isTrue);
    expect(gate.request(OverlayKind.questions), isFalse);
    gate.release(OverlayKind.ask);
    await pumpEventQueue();
    expect(freed, [OverlayKind.questions]);
  });

  test('a prompt waits only for another prompt', () async {
    final gate = OverlayGate();
    addTearDown(gate.dispose);
    final freed = <OverlayKind>[];
    gate.freed.listen(freed.add);

    expect(gate.request(OverlayKind.composer), isTrue);
    expect(gate.request(OverlayKind.ask), isTrue);
    expect(gate.showing, OverlayKind.ask);
    expect(gate.request(OverlayKind.ask), isFalse);

    gate.release(OverlayKind.ask);
    await pumpEventQueue();
    expect(freed, [OverlayKind.ask]);
    expect(gate.showing, OverlayKind.ask);
    gate.release(OverlayKind.ask);
    expect(gate.showing, OverlayKind.composer);
  });

  test('a prompt shows over the questions sheet', () {
    final gate = OverlayGate();
    addTearDown(gate.dispose);
    expect(gate.request(OverlayKind.questions), isTrue);
    expect(gate.request(OverlayKind.ask), isTrue);
    expect(gate.showing, OverlayKind.ask);
    gate.release(OverlayKind.ask);
    expect(gate.showing, OverlayKind.questions);
  });

  test('a waiting request released before its turn is dropped', () async {
    final gate = OverlayGate();
    addTearDown(gate.dispose);
    final freed = <OverlayKind>[];
    gate.freed.listen(freed.add);

    gate.request(OverlayKind.composer);
    expect(gate.request(OverlayKind.questions), isFalse);
    gate.release(OverlayKind.questions);
    gate.release(OverlayKind.composer);
    await pumpEventQueue();
    expect(freed, isEmpty);
    expect(gate.showing, isNull);
  });

  test('questions stay waiting while a prompt follows the composer', () async {
    final gate = OverlayGate();
    addTearDown(gate.dispose);
    final freed = <OverlayKind>[];
    gate.freed.listen(freed.add);

    gate.request(OverlayKind.composer);
    gate.request(OverlayKind.questions);
    gate.request(OverlayKind.ask);
    gate.release(OverlayKind.composer);
    await pumpEventQueue();
    expect(freed, isEmpty);
    gate.release(OverlayKind.ask);
    await pumpEventQueue();
    expect(freed, [OverlayKind.questions]);
  });

  test('a request for what already shows is granted, and listeners hear '
      'each change', () {
    final gate = OverlayGate();
    addTearDown(gate.dispose);
    var changes = 0;
    gate.addListener(() => changes++);
    expect(gate.request(OverlayKind.composer), isTrue);
    expect(gate.request(OverlayKind.composer), isTrue);
    expect(changes, 1);
    gate.release(OverlayKind.composer);
    expect(changes, 2);
    gate.release(OverlayKind.composer);
    expect(changes, 2);
    expect(gate.showing, isNull);
  });

  test('a waiting prompt is granted before waiting questions', () async {
    final gate = OverlayGate();
    addTearDown(gate.dispose);
    final freed = <OverlayKind>[];
    gate.freed.listen(freed.add);

    gate.request(OverlayKind.ask);
    expect(gate.request(OverlayKind.questions), isFalse);
    expect(gate.request(OverlayKind.ask), isFalse);
    gate.release(OverlayKind.ask);
    await pumpEventQueue();
    expect(freed, [OverlayKind.ask]);
    gate.release(OverlayKind.ask);
    await pumpEventQueue();
    expect(freed, [OverlayKind.ask, OverlayKind.questions]);
  });
}
