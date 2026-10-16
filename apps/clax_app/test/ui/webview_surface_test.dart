import 'package:clax_app/ui/webview_surface.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:webview_flutter/webview_flutter.dart';

void main() {
  final base = Uri.parse('http://127.0.0.1:7999');
  late List<Uri> opened;
  setUp(() => opened = []);

  NavigationDecision top(String url) => WebViewAppSurface.decide(
    base,
    url,
    isMainFrame: true,
    openExternal: opened.add,
  );

  test('the top level may load the shell app view', () {
    for (final path in [
      '/a/abcdefghjkmn?app=1',
      '/a/abcdefghjkmn/?app=1',
      '/a/abcdefghjkmn/v/3?app=1#top',
      '/a/abcdefghjkmn/about.html?app=1',
      '/a/abcdefghjkmn/v/3/pages/about.html?app=1',
    ]) {
      expect(
        top('http://127.0.0.1:7999$path'),
        NavigationDecision.navigate,
        reason: path,
      );
    }
    expect(opened, isEmpty);
  });

  test('anything else at the top level is refused', () {
    for (final url in [
      'about:blank',
      'http://127.0.0.1:7999/a/abcdefghjkmn',
      'http://127.0.0.1:7999/a/x?app=1',
      'http://127.0.0.1:7999/',
      'http://127.0.0.1:7999/api/token?app=1',
      'http://abc.localhost:7999/index.html',
      'http://localhost:7999/a/abcdefghjkmn?app=1',
      'http://127.0.0.1:7480/a/abcdefghjkmn?app=1',
      'file:///etc/passwd',
      'javascript:alert(1)',
    ]) {
      expect(top(url), NavigationDecision.prevent, reason: url);
    }
    expect(opened, isEmpty, reason: 'none of those is a web link outside');
  });

  test('a web or mail link goes outside the app', () {
    expect(top('https://example.com/x'), NavigationDecision.prevent);
    expect(top('mailto:a@example.com'), NavigationDecision.prevent);
    expect(opened, [
      Uri.parse('https://example.com/x'),
      Uri.parse('mailto:a@example.com'),
    ]);
  });

  test('the top level stays on its own artifact: another artifact\'s app '
      'view is refused and handed to openArtifact', () {
    final others = <String>[];
    NavigationDecision as(String url) => WebViewAppSurface.decide(
      base,
      url,
      isMainFrame: true,
      own: 'abcdefghjkmn',
      openExternal: opened.add,
      openArtifact: others.add,
    );
    expect(as('$base/a/abcdefghjkmn/v/2?app=1'), NavigationDecision.navigate);
    expect(as('$base/a/zyxwvtsrqpnm?app=1'), NavigationDecision.prevent);
    expect(others, ['zyxwvtsrqpnm']);
    expect(opened, isEmpty);
  });

  test('frames inside the page navigate freely', () {
    expect(
      WebViewAppSurface.decide(
        base,
        'http://abc.localhost:7999/index.html',
        isMainFrame: false,
      ),
      NavigationDecision.navigate,
    );
  });
}
