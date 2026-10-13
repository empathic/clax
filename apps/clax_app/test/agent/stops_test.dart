import 'dart:io';

import 'package:clax_app/agent/stops.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('stopped threads outlive the store: a new one reads them back', () async {
    final dir = await Directory.systemTemp.createTemp('stops');
    addTearDown(() => dir.delete(recursive: true));
    final file = File('${dir.path}/stopped.json');
    final a = FileStoppedThreads(file);
    expect(await a.all(), isEmpty);
    await a.put('t1', 'c1');
    await a.put('t2', 'c4');
    await a.remove('t2');
    expect(await FileStoppedThreads(file).all(), {'t1': 'c1'});
    await file.writeAsString('not json');
    expect(await FileStoppedThreads(file).all(), isEmpty);
  });
}
