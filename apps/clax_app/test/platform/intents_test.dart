import 'package:clax_app/platform/intents.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  const channel = MethodChannel(ChannelIntents.channelName);

  Future<void> platformSays(MethodCall call) => messenger.handlePlatformMessage(
    ChannelIntents.channelName,
    const StandardMethodCodec().encodeMethodCall(call),
    (_) {},
  );

  group('ChannelIntents', () {
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

    test('initial answers the intents run before Dart asked', () async {
      initial = ['comment', 7, ''];
      final intents = ChannelIntents();
      expect(await intents.initial(), ['comment']);
      expect(calls, ['initial']);
    });

    test('each intent run later arrives on the stream', () async {
      final intents = ChannelIntents();
      final got = <String>[];
      final sub = intents.intents.listen(got.add);
      await platformSays(const MethodCall('intent', 'comment'));
      await platformSays(const MethodCall('intent', 3));
      await platformSays(const MethodCall('other', 'comment'));
      await pumpEventQueue();
      expect(got, ['comment']);
      await sub.cancel();
    });

    test('without the platform half there are none', () async {
      messenger.setMockMethodCallHandler(channel, null);
      expect(await ChannelIntents().initial(), isEmpty);
    });
  });
}
