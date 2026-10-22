import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter_test/flutter_test.dart';

import '../integration_test/newer.dart';

Ready _ready(int version) => Ready(
  artifact: '7q3k9mzx2b4t',
  version: version,
  latest: version,
  file: 'index.html',
);

void main() {
  test('a page that opened on the version counts as shown, not as nothing', () {
    expect(newerSeen([_ready(3), _ready(4)], 4), NewerSeen.shown);
  });

  test('a newer offer of the version counts as offered', () {
    expect(newerSeen([_ready(3), const NewerMsg(4)], 4), NewerSeen.offered);
  });

  test('nothing yet while only older versions were shown or offered', () {
    expect(newerSeen([_ready(3), const NewerMsg(3)], 4), isNull);
  });
}
