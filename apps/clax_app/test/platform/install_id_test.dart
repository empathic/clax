import 'dart:io';
import 'dart:math';

import 'package:clax_app/platform/install_id.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  late Directory dir;
  setUp(() => dir = Directory.systemTemp.createTempSync('install-id-'));
  tearDown(() => dir.deleteSync(recursive: true));

  test('an install ID is made once and kept', () async {
    final first = await loadInstallId(dir, random: Random(1));
    expect(
      first,
      matches(RegExp(r'^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$')),
    );
    expect(await loadInstallId(dir, random: Random(2)), first);
    expect(File('${dir.path}/install-id').readAsStringSync(), first);
  });

  test('a file that holds no UUID is replaced', () async {
    File('${dir.path}/install-id').writeAsStringSync('not an ID');
    final id = await loadInstallId(dir, random: Random(3));
    expect(id, isNot('not an ID'));
    expect(await loadInstallId(dir), id);
  });
}
