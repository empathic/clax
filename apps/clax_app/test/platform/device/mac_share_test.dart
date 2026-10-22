import 'package:clax_app/platform/device/mac_share.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  late List<MethodCall> calls;
  late Object? answer;

  setUp(() {
    calls = [];
    answer = true;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MacShare.channel, (call) async {
          calls.add(call);
          return answer;
        });
  });

  tearDown(
    () => TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(MacShare.channel, null),
  );

  test('text goes to the native picker, which says whether a target was '
      'chosen', () async {
    expect(await const MacShare().text('hello'), isTrue);
    answer = false;
    expect(await const MacShare().text('again'), isFalse);
    expect(calls.map((c) => c.method), ['share', 'share']);
    expect(calls.map((c) => c.arguments), [
      {'text': 'hello'},
      {'text': 'again'},
    ]);
  });

  test('a file goes as its name and bytes', () async {
    final bytes = Uint8List.fromList([1, 2, 3]);
    expect(await const MacShare().file('a.txt', 'text/plain', bytes), isTrue);
    expect(calls.single.method, 'share');
    expect(calls.single.arguments, {'name': 'a.txt', 'data': bytes});
  });

  test('no answer is no target', () async {
    answer = null;
    expect(await const MacShare().text('hello'), isFalse);
  });
}
