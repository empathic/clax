// The render rig itself: a page with one word, rendered in every form and
// theme into CLAX_SHOTS_DIR (skipped without it), and the forms checked
// against the spec's three sizes on every run.
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

void main() {
  test('shotsDir is null without CLAX_SHOTS_DIR, and the forms are the '
      'phone, Android wide and Mac sizes', () {
    expect(shotsDirFrom({}), isNull);
    expect(shotsDirFrom({'CLAX_SHOTS_DIR': ''}), isNull);
    expect(shotsDirFrom({'CLAX_SHOTS_DIR': '/tmp/s'}), '/tmp/s');
    expect(allForms.map((f) => f.name), ['phone', 'wide', 'mac']);
    expect(phone.size, const Size(412, 915));
    expect(phone.dpr, 2.625);
    expect(phone.platform, TargetPlatform.android);
    expect((phone.top, phone.bottom, phone.toolbar), (24.0, 16.0, 0.0));
    expect(wide.size, const Size(1280, 800));
    expect(wide.dpr, 2);
    expect(wide.platform, TargetPlatform.android);
    expect((wide.top, wide.bottom, wide.toolbar), (24.0, 16.0, 0.0));
    expect(mac.size, const Size(1200, 800));
    expect(mac.dpr, 2);
    expect(mac.platform, TargetPlatform.macOS);
    expect((mac.top, mac.bottom, mac.toolbar), (0.0, 0.0, 52.0));
    expect(shotName('rig', phone, false, 1), 'rig-phone-light.png');
    expect(shotName('rig', mac, true, 1), 'rig-mac-dark.png');
    expect(shotName('rig', wide, false, 1.3), 'rig-wide-light-130.png');
    expect(shotName('rig', phone, true, 2), 'rig-phone-dark-200.png');
  });

  shots('rig', (s) async {
    await s.tester.pumpWidget(
      s.app(const Scaffold(body: Center(child: Text('Clax')))),
    );
    await s.settle();
    await s.take('rig');
  });
}
