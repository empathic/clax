import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// Puts the owner's key into an in-process daemon's memory
/// (`PUT /api/sample/key`); null clears it. True on 204.
abstract interface class SampleKeyPort {
  Future<bool> put(Uri base, String token, String? key);
}

/// [SampleKeyPort] over HTTP, with the daemon's bearer token. A put gives
/// up after [timeout] and answers false; nothing about the key is logged.
class HttpSampleKeyPort implements SampleKeyPort {
  HttpSampleKeyPort({
    HttpClient Function()? client,
    this.timeout = const Duration(seconds: 5),
  }) : _client = client ?? HttpClient.new;

  final HttpClient Function() _client;
  final Duration timeout;

  @override
  Future<bool> put(Uri base, String token, String? key) async {
    final client = _client();
    try {
      return await () async {
        final req = await client.putUrl(base.resolve('/api/sample/key'));
        req.headers
          ..set(HttpHeaders.authorizationHeader, 'Bearer $token')
          ..contentType = ContentType.json;
        req.write(jsonEncode({'key': key}));
        final res = await req.close();
        await res.drain<void>();
        return res.statusCode == HttpStatus.noContent;
      }().timeout(timeout);
    } on Object {
      return false;
    } finally {
      client.close(force: true);
    }
  }
}
