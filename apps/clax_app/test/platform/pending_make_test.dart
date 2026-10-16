import 'dart:io';

import 'package:clax_app/platform/pending_make.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late Directory dir;
  setUp(() => dir = Directory.systemTemp.createTempSync('pending-make-'));
  tearDown(() => dir.deleteSync(recursive: true));

  test('a saved make is offered once by a later instance', () async {
    final file = File('${dir.path}/pending-make.json');
    await PendingMake(file).save('a tracker for my runs');
    final next = PendingMake(file);
    expect(await next.take(), 'a tracker for my runs');
    expect(await next.take(), isNull);
    expect(file.existsSync(), isFalse);
  });

  test('a make that ended leaves nothing, whatever the write order', () async {
    final file = File('${dir.path}/pending-make.json');
    final p = PendingMake(file);
    // Not awaited one by one, as the app sends them.
    final writes = [p.save('one'), p.clear(), p.save('two'), p.clear()];
    await Future.wait(writes);
    expect(await PendingMake(file).take(), isNull);
  });

  test("a save queued while the last run's make is taken is kept", () async {
    final file = File('${dir.path}/pending-make.json');
    await PendingMake(file).save('old');
    final p = PendingMake(file);
    final taken = p.take();
    final saved = p.save('new');
    expect(await taken, 'old');
    await saved;
    expect(await PendingMake(file).take(), 'new');
  });

  test('a torn file offers nothing', () async {
    final file = File('${dir.path}/pending-make.json')
      ..writeAsStringSync('{"requ');
    expect(await PendingMake(file).take(), isNull);
  });

  test('in memory, it keeps the request for this run only', () async {
    final p = PendingMake();
    await p.save('x');
    expect(await p.take(), 'x');
    expect(await PendingMake().take(), isNull);
  });

  test('several makes at once are each kept until each ends', () async {
    final file = File('${dir.path}/pending-make.json');
    final p = PendingMake(file);
    await p.save('runs');
    await p.save('habits');
    await p.save('books');
    await p.clear('habits');
    expect(await PendingMake(file).takeAll(), ['runs', 'books']);
    expect(await PendingMake(file).takeAll(), isEmpty);
  });

  test("an earlier version's file offers its one request", () async {
    final file = File('${dir.path}/pending-make.json')
      ..writeAsStringSync('{"request": "a tracker"}');
    expect(await PendingMake(file).takeAll(), ['a tracker']);
  });

  test('a make waiting for the connection is kept as waiting, across a '
      'restart, until it goes on or ends', () async {
    final file = File('${dir.path}/pending-make.json');
    final p = PendingMake(file);
    await p.save('runs');
    await p.save('habits');
    await p.waiting('habits', true);
    await p.save('books');
    await p.waiting('books', true);
    await p.waiting('books', false);
    expect(await PendingMake(file).takeEach(), [
      (request: 'runs', waiting: false),
      (request: 'habits', waiting: true),
      (request: 'books', waiting: false),
    ]);

    final next = PendingMake(file);
    await next.save('runs');
    await next.waiting('runs', true);
    await next.clear('runs');
    expect(await PendingMake(file).takeEach(), isEmpty);
  });
}
