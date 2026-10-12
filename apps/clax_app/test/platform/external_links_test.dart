import 'package:clax_app/platform/external_links.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('dev.empathic.clax/native');
  late List<MethodCall> calls;

  setUp(() {
    calls = [];
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          calls.add(call);
          return true;
        });
  });
  tearDown(
    () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null),
  );

  test('web and mail links go to the native openExternal', () async {
    const links = ExternalLinks();
    expect(await links.open(Uri.parse('https://example.com/a?b=1')), isTrue);
    expect(await links.open(Uri.parse('mailto:someone@example.com')), isTrue);
    expect([for (final c in calls) c.method], ['openExternal', 'openExternal']);
    expect(calls.first.arguments, {'url': 'https://example.com/a?b=1'});
  });

  test('other schemes are refused without a call', () async {
    const links = ExternalLinks();
    for (final u in ['file:///etc/passwd', 'intent://x', 'javascript:alert(1)', 'tel:123']) {
      expect(await links.open(Uri.parse(u)), isFalse, reason: u);
    }
    expect(calls, isEmpty);
  });

  test('a platform without the method answers false', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          throw MissingPluginException();
        });
    expect(await const ExternalLinks().open(Uri.parse('https://example.com')), isFalse);
  });
}
