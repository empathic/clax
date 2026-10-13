import 'package:clax_app/daemon/daemon_key.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:flutter_test/flutter_test.dart';

class MemoryKey implements SecureKey {
  String? v;
  @override
  Future<String?> read() async => v;
  @override
  Future<void> write(String? key) async {
    final t = key?.trim();
    v = (t == null || t.isEmpty) ? null : t;
  }
}

void main() {
  test(
    'a saved or removed key restarts the daemon; the same key does not',
    () async {
      final inner = MemoryKey();
      var restarts = 0;
      final key = DaemonKey(inner, onChanged: () async => restarts++);
      await key.write('sk-a');
      expect((await key.read(), restarts), ('sk-a', 1));
      await key.write(' sk-a ');
      expect(restarts, 1);
      await key.write('sk-b');
      expect(restarts, 2);
      await key.write(null);
      expect((await key.read(), restarts), (null, 3));
    },
  );
}
