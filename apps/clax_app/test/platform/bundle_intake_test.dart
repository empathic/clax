import 'package:clax_app/platform/bundle_intake.dart';
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
      final got = <OpenedBundle>[];
      final sub = intake.opened.listen(got.add);
      await pumpEventQueue();
      await platformSays(
        const MethodCall('opened', {'path': '/Users/x/Plants.clax'}),
      );
      await pumpEventQueue();
      expect(got, const [
        OpenedBundle.file('/cache/incoming/a.clax'),
        OpenedBundle.refused('too_large'),
        OpenedBundle.file('/Users/x/Plants.clax'),
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
        expect(got, const OpenedBundle.file('/p.clax'));
      },
    );

    test('malformed events are dropped', () async {
      initial = [
        {'path': ''},
        'nope',
        {'other': 1},
      ];
      final intake = ChannelBundleIntake();
      final got = <OpenedBundle>[];
      final sub = intake.opened.listen(got.add);
      await pumpEventQueue();
      await platformSays(const MethodCall('opened', {'path': 3}));
      await platformSays(const MethodCall('dance', {'path': '/x.clax'}));
      await platformSays(const MethodCall('opened', {'path': '/y.clax'}));
      await pumpEventQueue();
      expect(got, const [OpenedBundle.file('/y.clax')]);
      await sub.cancel();
    });

    test('a platform without the channel opens nothing', () async {
      messenger.setMockMethodCallHandler(channel, null);
      final intake = ChannelBundleIntake();
      final got = <OpenedBundle>[];
      final sub = intake.opened.listen(got.add);
      await pumpEventQueue();
      expect(got, isEmpty);
      await sub.cancel();
    });
  });

  test('FakeBundleIntake delivers what the test opens', () async {
    final intake = FakeBundleIntake();
    intake.open('/a.clax');
    final got = <OpenedBundle>[];
    final sub = intake.opened.listen(got.add);
    intake.refuse('too_large');
    await pumpEventQueue();
    expect(got, const [
      OpenedBundle.file('/a.clax'),
      OpenedBundle.refused('too_large'),
    ]);
    await sub.cancel();
  });
}
