import 'package:clax_app/daemon/sample_key.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/scripted_server.dart';

void main() {
  late ScriptedServer server;
  setUp(() async => server = await ScriptedServer.start());
  tearDown(() => server.close());

  test('puts the key with the bearer token and is true on 204', () async {
    server.handle('PUT', '/api/sample/key', (_, res) async {
      res.statusCode = 204;
      await res.close();
    });
    final port = HttpSampleKeyPort();
    expect(await port.put(server.base, 'tok', 'sk-1'), isTrue);
    expect(await port.put(server.base, 'tok', null), isTrue);
    final reqs = server.to('PUT', '/api/sample/key');
    expect(reqs, hasLength(2));
    expect(reqs.first.headers['authorization'], 'Bearer tok');
    expect(reqs.first.headers['content-type'], startsWith('application/json'));
    expect(reqs.first.json, {'key': 'sk-1'});
    expect(reqs.last.json, {'key': null});
  });

  test('is false on any other answer, and when nothing answers', () async {
    server.json('PUT', '/api/sample/key', 404, {
      'error': {'code': 'not_found'},
    });
    final port = HttpSampleKeyPort();
    expect(await port.put(server.base, 'tok', 'sk-1'), isFalse);
    final base = server.base;
    await server.close();
    expect(await port.put(base, 'tok', 'sk-1'), isFalse);
  });
}
