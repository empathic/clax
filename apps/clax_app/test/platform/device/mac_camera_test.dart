import 'package:clax_app/platform/device/device_plugins.dart';
import 'package:clax_app/platform/device/device_ports.dart';
import 'package:clax_app/platform/device/mac_camera.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  late List<MethodCall> calls;
  late Object? Function(MethodCall) answer;

  setUp(() {
    calls = [];
    answer = (_) => null;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MacCamera.channel, (call) async {
          calls.add(call);
          return answer(call);
        });
  });
  tearDown(
    () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MacCamera.channel, null),
  );

  Future<DeviceError> refusal(Future<Object?> call) async {
    try {
      await call;
    } on DeviceError catch (e) {
      return e;
    }
    fail('take did not reject');
  }

  test('take asks the capture sheet for a photo at most CameraPort.maxSide '
      'a side and gives its JPEG', () async {
    final jpeg = Uint8List.fromList([0xff, 0xd8, 0xff, 0xd9]);
    answer = (_) => {'data': jpeg, 'width': 1600, 'height': 1200};
    final photo = await const MacCamera().take();
    expect(calls.single.method, 'take');
    expect(calls.single.arguments, {'maxSide': CameraPort.maxSide});
    expect(photo!.mime, 'image/jpeg');
    expect(photo.data, jpeg);
    expect((photo.width, photo.height), (1600, 1200));
  });

  test('a closed sheet is a cancel: null', () async {
    expect(await const MacCamera().take(), isNull);
  });

  test("the sheet's refusals are the device codes", () async {
    for (final code in ['unavailable', 'not_granted']) {
      answer = (_) => throw PlatformException(code: code, message: 'why $code');
      final e = await refusal(const MacCamera().take());
      expect((e.code, e.message), (code, 'why $code'));
    }
    // Anything else the sheet fails with is unavailable.
    answer = (_) => throw PlatformException(code: 'session_failed');
    expect((await refusal(const MacCamera().take())).code, 'unavailable');
  });

  test('without the native side, take is unavailable', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MacCamera.channel, null);
    expect((await refusal(const MacCamera().take())).code, 'unavailable');
  });

  test('a malformed answer is unavailable', () async {
    answer = (_) => {'data': 'not bytes'};
    expect((await refusal(const MacCamera().take())).code, 'unavailable');
  });

  test("on macOS PluginCamera's take is the capture sheet's", () async {
    answer = (_) => {
      'data': Uint8List.fromList([1]),
      'width': 2,
      'height': 3,
    };
    final photo = await PluginCamera(platform: TargetPlatform.macOS).take();
    expect((photo!.width, photo.height), (2, 3));
    expect(calls.single.method, 'take');
  });
}
