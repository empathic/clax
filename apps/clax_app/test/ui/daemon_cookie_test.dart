import 'dart:async';

import 'package:clax_app/ui/daemon_cookie.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:webview_flutter/webview_flutter.dart';

/// The cookie store's writes and the WebView's loads, in order.
final log = <String>[];

/// A cookie store whose writes complete only when the test says so.
class _Store {
  final pending = <Completer<void>>[];
  final cookies = <WebViewCookie>[];

  Future<void> set(WebViewCookie c) {
    log.add('cookie ${c.value}');
    cookies.add(c);
    final done = Completer<void>();
    pending.add(done);
    return done.future.then((_) => log.add('cookie set ${c.value}'));
  }

  void completeAll() {
    for (final c in pending) {
      if (!c.isCompleted) c.complete();
    }
  }
}

void main() {
  final base = Uri.parse('http://127.0.0.1:7999');
  late _Store store;
  String? token;
  DaemonKeyCookie key() => DaemonKeyCookie(
    token: () => token,
    setCookie: store.set,
    platform: TargetPlatform.macOS,
  );

  setUp(() {
    log.clear();
    store = _Store();
    token = 'tok';
  });

  group('the key cookie', () {
    test('is for the daemon host on WKWebView and its full URL on Android', () {
      final mac = DaemonKeyCookie.cookieFor(base, 'tok', TargetPlatform.macOS);
      expect(
        (mac.name, mac.value, mac.domain, mac.path),
        ('clax_key_7999', 'tok', '127.0.0.1', '/'),
      );
      final ios = DaemonKeyCookie.cookieFor(base, 'tok', TargetPlatform.iOS);
      expect(ios.domain, '127.0.0.1');
      final android = DaemonKeyCookie.cookieFor(
        base,
        'tok',
        TargetPlatform.android,
      );
      expect(
        (android.name, android.value, android.domain, android.path),
        ('clax_key_7999', 'tok', 'http://127.0.0.1:7999/', '/'),
      );
    });

    test('ready sets the current token, and nothing without one', () async {
      final k = key();
      store.completeAll();
      token = null;
      await k.ready(base);
      expect(store.cookies, isEmpty);
      token = 'new';
      final f = k.ready(base);
      store.completeAll();
      await f;
      expect(store.cookies.single.value, 'new');
    });

    test('a refresh with a new token tells restarts once it is set', () async {
      final k = key();
      final heard = <String>[];
      k.restarts.listen((_) => heard.add(log.last));
      final first = k.refresh(base, 'a');
      store.completeAll();
      await first;
      final same = k.refresh(base, 'a');
      store.completeAll();
      await same;
      await pumpEventQueue();
      expect(heard, isEmpty, reason: 'the first start and the same token');
      final next = k.refresh(base, 'b');
      store.completeAll();
      await next;
      await pumpEventQueue();
      expect(heard, ['cookie set b']);
    });
  });

  group('a daemon WebView\'s loads', () {
    CookieLoads loads(DaemonKeyCookie k, {bool followRestarts = false}) =>
        CookieLoads(
          k,
          base,
          load: (url) async => log.add('load $url'),
          reload: () async => log.add('reload'),
          followRestarts: followRestarts,
        );

    test('load and reload wait until the cookie is set', () async {
      final l = loads(key());
      final url = base.replace(path: '/inbox');
      final loading = l.load(url);
      await pumpEventQueue();
      expect(log, ['cookie tok'], reason: 'no load before the cookie is set');
      store.completeAll();
      await loading;
      expect(log, ['cookie tok', 'cookie set tok', 'load $url']);
      log.clear();
      token = 'new';
      final reloading = l.reload();
      await pumpEventQueue();
      expect(log, ['cookie new']);
      store.completeAll();
      await reloading;
      expect(log, ['cookie new', 'cookie set new', 'reload']);
      l.dispose();
    });

    test('without a key cookie they go straight through', () async {
      final l = CookieLoads(
        null,
        base,
        load: (url) async => log.add('load $url'),
        reload: () async => log.add('reload'),
      );
      await l.load(base);
      await l.reload();
      expect(log, ['load $base', 'reload']);
    });

    test(
      'a loaded page following restarts loads again with the new cookie',
      () async {
        final k = key();
        final unloaded = loads(k, followRestarts: true);
        final page = loads(k, followRestarts: true);
        final unfollowed = loads(k);
        final first = k.refresh(base, 'tok');
        store.completeAll();
        await first;
        for (final l in [page, unfollowed]) {
          final f = l.load(base);
          store.completeAll();
          await f;
        }
        log.clear();

        token = 'new';
        final restart = k.refresh(base, 'new');
        store.completeAll();
        await restart;
        await pumpEventQueue();
        store.completeAll();
        await pumpEventQueue();
        // Only the loaded page that follows restarts reloads, after the
        // new cookie is set.
        expect(log, [
          'cookie new',
          'cookie set new',
          'cookie new',
          'cookie set new',
          'reload',
        ]);

        log.clear();
        for (final l in [page, unloaded, unfollowed]) {
          l.dispose();
        }
        final again = k.refresh(base, 'newer');
        store.completeAll();
        await again;
        await pumpEventQueue();
        expect(log, ['cookie newer', 'cookie set newer']);
      },
    );
  });
}
