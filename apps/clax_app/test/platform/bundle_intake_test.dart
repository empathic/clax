import 'package:clax_app/platform/bundle_intake.dart';
import 'package:clax_app/ui/bundle_sheets.dart' show refusalMessage;
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  const channel = MethodChannel(ChannelBundleIntake.channelName);

  Future<void> platformSays(MethodCall call) => messenger.handlePlatformMessage(
    ChannelBundleIntake.channelName,
    const StandardMethodCodec().encodeMethodCall(call),
    (_) {},
  );

  group('ChannelBundleIntake', () {
    late List<String> calls;
    late Object? initial;
    setUp(() {
      calls = [];
      initial = null;
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add(call.method);
        return call.method == 'initial' ? initial : null;
      });
    });
    tearDown(() => messenger.setMockMethodCallHandler(channel, null));

    test('the launching files come first, then each one opened', () async {
      initial = [
        {'path': '/cache/incoming/a.clax'},
        {'refused': 'too_large'},
      ];
      final intake = ChannelBundleIntake();
      final got = <BundleIntakeEvent>[];
      final sub = intake.opened.listen(got.add);
      await pumpEventQueue();
      await platformSays(
        const MethodCall('opened', {'path': '/Users/x/Plants.clax'}),
      );
      await pumpEventQueue();
      expect(got, const [
        BundleIntakeEvent.file('/cache/incoming/a.clax'),
        BundleIntakeEvent.refused('too_large'),
        BundleIntakeEvent.file('/Users/x/Plants.clax'),
      ]);
      expect(calls, ['initial']);
      await sub.cancel();
    });

    test(
      'a file opened before anyone listens is kept for the listener',
      () async {
        final intake = ChannelBundleIntake();
        await pumpEventQueue();
        await platformSays(const MethodCall('opened', {'path': '/p.clax'}));
        final got = await intake.opened.first;
        expect(got, const BundleIntakeEvent.file('/p.clax'));
      },
    );

    test('malformed events are dropped', () async {
      initial = [
        {'path': ''},
        'nope',
        {'other': 1},
      ];
      final intake = ChannelBundleIntake();
      final got = <BundleIntakeEvent>[];
      final sub = intake.opened.listen(got.add);
      await pumpEventQueue();
      await platformSays(const MethodCall('opened', {'path': 3}));
      await platformSays(const MethodCall('dance', {'path': '/x.clax'}));
      await platformSays(const MethodCall('opened', {'path': '/y.clax'}));
      await pumpEventQueue();
      expect(got, const [BundleIntakeEvent.file('/y.clax')]);
      await sub.cancel();
    });

    test('a platform without the channel opens nothing', () async {
      messenger.setMockMethodCallHandler(channel, null);
      final intake = ChannelBundleIntake();
      final got = <BundleIntakeEvent>[];
      final sub = intake.opened.listen(got.add);
      await pumpEventQueue();
      expect(got, isEmpty);
      await sub.cancel();
    });
  });

  group('Get in Clax links', () {
    const id = '7q3k9mzx2b4t';

    test('intake_parses_clax_get_and_https_get', () {
      expect(
        BundleIntakeEvent.artifactOf('https://clax.dev/a/$id/get'),
        Uri.parse('https://clax.dev/a/$id'),
      );
      expect(
        BundleIntakeEvent.artifactOf('https://CLAX.dev/a/$id/get/?ref=qr#x'),
        Uri.parse('https://clax.dev/a/$id'),
      );
      final inner = Uri.encodeQueryComponent('https://clax.example.org/a/$id');
      expect(
        BundleIntakeEvent.artifactOf('clax:get?url=$inner'),
        Uri.parse('https://clax.example.org/a/$id'),
      );
      final ported = Uri.encodeQueryComponent('https://h.example:8443/a/$id/');
      expect(
        BundleIntakeEvent.artifactOf('clax:get?url=$ported'),
        Uri.parse('https://h.example:8443/a/$id'),
      );
      // Punycode, IPv4 and IPv6 hosts pass.
      for (final host in [
        'xn--clx-8cd.dev',
        '192.168.1.20',
        '127.0.0.1',
        '[::1]:7480',
        'localhost',
      ]) {
        final u = Uri.encodeQueryComponent('https://$host/a/$id');
        expect(
          BundleIntakeEvent.artifactOf('clax:get?url=$u'),
          Uri.parse('https://$host/a/$id'),
          reason: host,
        );
      }
      expect(
        BundleIntakeEvent.of({'url': 'https://clax.dev/a/$id/get'}),
        BundleIntakeEvent.url(Uri.parse('https://clax.dev/a/$id')),
      );
    });

    test('intake_refuses_other_urls', () {
      String get(String inner) =>
          'clax:get?url=${Uri.encodeQueryComponent(inner)}';
      for (final link in [
        '',
        'not a link',
        'https://clax.dev/a/$id',
        'https://clax.dev/a/$id/edit',
        'https://clax.dev/a/$id/get/more',
        'https://clax.dev/b/$id/get',
        'https://clax.dev/a/7Q3K9MZX2B4T/get',
        'https://clax.dev/a/7q3k9mzx2b4/get',
        'https://clax.dev/a/7q3k9mzx2b4i/get',
        'https://clax.dev/a/%2e%2e/get',
        'http://clax.dev/a/$id/get',
        'https://evil.example/a/$id/get',
        'https://clax.dev.evil.example/a/$id/get',
        'https://user@clax.dev/a/$id/get',
        'https://clax.dev:8443/a/$id/get',
        'file:///a/$id/get',
        'content://x/a/$id/get',
        'javascript:alert(1)',
        'clax:get',
        'clax:get?url=',
        'clax:open?url=${Uri.encodeQueryComponent('https://clax.dev/a/$id')}',
        'clax://get?url=${Uri.encodeQueryComponent('https://clax.dev/a/$id')}',
        get('http://clax.dev/a/$id'),
        get('https://clax.dev/a/$id/get'),
        get('https://clax.dev/x/$id'),
        get('https://u:p@clax.dev/a/$id'),
        get('https:/a/$id'),
        get('clax:get?url=https://clax.dev/a/$id'),
        get('https://clax.dev/a/$id?x=1'),
        get('https://clax.dev/a/$id?'),
        get('https://clax.dev/a/$id#top'),
        get('https://clax.dev%2F.evil.example/a/$id'),
        get('https://clax.dev%252F.evil.example/a/$id'),
        get('https://cl%D0%B0x.dev/a/$id'),
        get('https://clаx.dev/a/$id'),
        get('https://ｃlax.dev/a/$id'),
        get('https://[fe80::1%25en0]/a/$id'),
        '${get('https://clax.dev/a/$id')}&url=${Uri.encodeQueryComponent('https://e.example/a/$id')}',
      ]) {
        expect(BundleIntakeEvent.artifactOf(link), isNull, reason: link);
        expect(
          BundleIntakeEvent.of({'url': link}),
          const BundleIntakeEvent.refused('not_a_link'),
          reason: link,
        );
      }
      expect(BundleIntakeEvent.of({'url': 3}), isNull);
    });

    test('a refused link says so', () {
      expect(refusalMessage('not_a_link'), 'That link isn’t a Clax app.');
    });

    test('the channel delivers accepted links and refuses the rest', () async {
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      messenger.setMockMethodCallHandler(
        channel,
        (call) async => call.method == 'initial'
            ? [
                {'url': 'https://clax.dev/a/$id/get'},
                {'url': 'https://evil.example/a/$id/get'},
              ]
            : null,
      );
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
      final intake = ChannelBundleIntake();
      final got = <BundleIntakeEvent>[];
      final sub = intake.opened.listen(got.add);
      await pumpEventQueue();
      await platformSays(
        const MethodCall('opened', {'url': 'clax:get?url=nope'}),
      );
      await platformSays(
        MethodCall('opened', {
          'url':
              'clax:get?url=${Uri.encodeQueryComponent('https://o.example/a/$id')}',
        }),
      );
      await pumpEventQueue();
      expect(got, [
        BundleIntakeEvent.url(Uri.parse('https://clax.dev/a/$id')),
        const BundleIntakeEvent.refused('not_a_link'),
        const BundleIntakeEvent.refused('not_a_link'),
        BundleIntakeEvent.url(Uri.parse('https://o.example/a/$id')),
      ]);
      await sub.cancel();
    });

    test('FakeBundleIntake refuses what the platform intake refuses', () async {
      final intake = FakeBundleIntake();
      final got = <BundleIntakeEvent>[];
      final sub = intake.opened.listen(got.add);
      intake.openLink('https://evil.example/a/$id/get');
      intake.openLink('https://clax.dev/a/$id/get');
      await pumpEventQueue();
      expect(got, [
        const BundleIntakeEvent.refused('not_a_link'),
        BundleIntakeEvent.url(Uri.parse('https://clax.dev/a/$id')),
      ]);
      await sub.cancel();
    });
  });

  test('FakeBundleIntake delivers what the test opens', () async {
    final intake = FakeBundleIntake();
    intake.open('/a.clax');
    final got = <BundleIntakeEvent>[];
    final sub = intake.opened.listen(got.add);
    intake.refuse('too_large');
    await pumpEventQueue();
    expect(got, const [
      BundleIntakeEvent.file('/a.clax'),
      BundleIntakeEvent.refused('too_large'),
    ]);
    await sub.cancel();
  });
}
