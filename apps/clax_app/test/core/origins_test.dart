import 'package:clax_app/core/origins.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  for (final android in [true, false]) {
    final origins = AppOrigins.forPlatform(android: android);
    final platform = android ? 'Android' : 'Apple';

    test("$platform's content template has one {id}, as its host's first label", () {
      // As crates/clax-embed/src/lib.rs `content_template_ok` requires of
      // embed.json, or the core does not open (no_origins).
      final t = origins.contentTemplate;
      expect('{id}'.allMatches(t), hasLength(1), reason: t);
      final authority = t.substring(t.indexOf('://') + 3);
      expect(authority, isNot(matches(RegExp('[/?#@]'))), reason: t);
      expect(authority.split(':').first.split('.').first, '{id}', reason: t);
      expect(origins.toJson()['content'], t);
    });

    test("$platform's content origins are one per artifact", () {
      const a = 'abcdefghjkmn', b = 'nmkjhgfedcba';
      expect(origins.content(a), isNot(origins.content(b)));
      expect(origins.idOfHost(Uri.parse(origins.content(a)).host), a);
    });
  }
}
