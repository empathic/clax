import 'dart:isolate';

import 'package:clax_app/ui/app_icon.dart';
import 'package:flutter_test/flutter_test.dart';

const _icon =
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
    '<rect width="1024" height="1024" fill="#f80"/></svg>';

/// The isolate it runs on, in place of cleaning.
String? _isolateName(String raw) => Isolate.current.debugName;

void main() {
  test('cleanIconsAway cleans as cleanIconsHere does', () async {
    final raw = {
      'plain': _icon,
      'script': _icon.replaceFirst('</svg>', '<script>x()</script></svg>'),
      'wide': _icon.replaceFirst('0 0 1024 1024', '0 0 1024 10'),
      'junk': 'not svg',
    };
    final here = await cleanIconsHere(raw);
    expect(await cleanIconsAway(raw), here);
    expect(here['plain'], contains('#f80'));
    expect(here['script'], isNot(contains('script')));
    expect(here['wide'], isNull);
  });

  test('cleanIconsAway cleans on another isolate, and nothing for none', () async {
    final names = await cleanIconsAway({'a': _icon}, clean: _isolateName);
    expect(names['a'], isNot(Isolate.current.debugName));
    expect(await cleanIconsAway(const {}), isEmpty);
  });
}
