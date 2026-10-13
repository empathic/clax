import 'package:clax_app/platform/app_notice.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_app_notice.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  const channel = MethodChannel(ChannelAppNotice.channelName);

  group('ChannelAppNotice', () {
    late List<MethodCall> calls;
    setUp(() {
      calls = [];
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add(call);
        return switch (call.method) {
          'allowed' => true,
          'launchTap' => {'artifact': 'a7', 'action': 'threads'},
          _ => null,
        };
      });
    });
    tearDown(() => messenger.setMockMethodCallHandler(channel, null));

    test('sends each call with its arguments', () async {
      final n = ChannelAppNotice();
      await n.show('a1', 'Runs');
      expect(await n.allowed(), isTrue);
      await n.clear();
      expect(await n.launchTap(), const AppNoticeTap('a7', AppAction.threads));
      expect(
        [for (final c in calls) '${c.method} ${c.arguments}'],
        [
          'show {artifact: a1, title: Runs}',
          'allowed null',
          'clear null',
          'launchTap null',
        ],
      );
    });

    Future<void> platformSays(MethodCall call) =>
        messenger.handlePlatformMessage(
          ChannelAppNotice.channelName,
          const StandardMethodCodec().encodeMethodCall(call),
          (_) {},
        );

    test(
      'each action of a tap arrives on taps; anything else is dropped',
      () async {
        final n = ChannelAppNotice();
        final got = <AppNoticeTap>[];
        final sub = n.taps.listen(got.add);
        for (final action in ['open', 'comment', 'threads', 'dance']) {
          await platformSays(
            MethodCall('tapped', {'artifact': 'a9', 'action': action}),
          );
        }
        await platformSays(const MethodCall('tapped', {'action': 'comment'}));
        await platformSays(
          const MethodCall('tapped', {'artifact': '', 'action': 'comment'}),
        );
        await pumpEventQueue();
        expect(got, const [
          AppNoticeTap('a9', AppAction.open),
          AppNoticeTap('a9', AppAction.comment),
          AppNoticeTap('a9', AppAction.threads),
        ]);
        await sub.cancel();
      },
    );

    test(
      'without the platform side nothing throws and nothing is allowed',
      () async {
        messenger.setMockMethodCallHandler(channel, null);
        final n = ChannelAppNotice();
        await n.show('a1', 'Runs');
        await n.clear();
        expect(await n.allowed(), isFalse);
        expect(await n.launchTap(), isNull);
      },
    );

    test('a platform error reads as not allowed', () async {
      messenger.setMockMethodCallHandler(
        channel,
        (call) async => throw PlatformException(code: 'x'),
      );
      expect(await ChannelAppNotice().allowed(), isFalse);
    });
  });

  group('AppNoticeSlot', () {
    test('a clear from a screen another replaced since does nothing', () async {
      final fake = FakeAppNotice();
      final slot = AppNoticeSlot(fake);
      final below = Object(), above = Object();
      slot.show(below, 'a1', 'Runs');
      slot.show(above, 'a2', 'Notes');
      slot.clear(below);
      expect(fake.shown, 'a2 Notes');
      slot.clear(above);
      expect(fake.shown, isNull);
      slot.clear(above);
      expect(fake.calls, ['show(a1, Runs)', 'show(a2, Notes)', 'clear']);
    });

    test('a failing allowed reads as not allowed', () async {
      expect(await AppNoticeSlot(_Throws()).allowed(), isFalse);
    });
  });
}

class _Throws extends FakeAppNotice {
  @override
  Future<bool> allowed() => Future.error(StateError('gone'));
}
