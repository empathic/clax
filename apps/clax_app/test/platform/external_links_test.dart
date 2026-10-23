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

  test('a sign-in page opens as auth, https only; closeAuth closes it', () async {
    const links = ExternalLinks();
    expect(
      await links.openAuth(Uri.parse('https://clax.dev/device?code=AB-CD')),
      isTrue,
    );
    expect(await links.openAuth(Uri.parse('http://clax.dev/device')), isFalse);
    expect(await links.openAuth(Uri.parse('mailto:a@example.com')), isFalse);
    await links.closeAuth();
    expect([for (final c in calls) c.method], ['openExternal', 'closeAuth']);
    expect(calls.first.arguments, {
      'url': 'https://clax.dev/device?code=AB-CD',
      'mode': 'auth',
    });
  });

  test('the device name is trimmed, and none where it is blank or missing', () async {
    String? name = '  Sam’s Pixel ';
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async => name);
    expect(await const ExternalLinks().deviceName(), 'Sam’s Pixel');
    name = ' ';
    expect(await const ExternalLinks().deviceName(), isNull);
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          throw MissingPluginException();
        });
    expect(await const ExternalLinks().deviceName(), isNull);
  });

  test('a platform without the method answers false', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          throw MissingPluginException();
        });
    expect(await const ExternalLinks().open(Uri.parse('https://example.com')), isFalse);
  });
}
