import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// Asks whether a daemon answers.
abstract interface class HealthProbe {
  /// Whether `GET <base>/healthz` answers 200.
  Future<bool> up(Uri base);

  /// The `pid` that `GET <base>/healthz` answers with, or null when it
  /// does not answer 200 with one.
  Future<int?> pid(Uri base);
}

/// [HealthProbe] over HTTP. One probe gives up after [timeout], so a
/// wedged daemon reads as down rather than stalling the supervisor.
class HttpHealthProbe implements HealthProbe {
  HttpHealthProbe({this.timeout = const Duration(seconds: 2)})
    : _client = HttpClient()..connectionTimeout = const Duration(seconds: 1);

  final Duration timeout;
  final HttpClient _client;

  @override
  Future<bool> up(Uri base) async => (await _get(base)) != null;

  @override
  Future<int?> pid(Uri base) async {
    final body = await _get(base);
    if (body == null) return null;
    try {
      final j = jsonDecode(body);
      final pid = j is Map<String, Object?> ? j['pid'] : null;
      return pid is int ? pid : null;
    } on FormatException {
      return null;
    }
  }

  /// The body of a 200 from `/healthz`, or null.
  Future<String?> _get(Uri base) async {
    try {
      return await () async {
        final req = await _client.getUrl(base.resolve('/healthz'));
        final res = await req.close();
        final body = await res.transform(utf8.decoder).join();
        return res.statusCode == 200 ? body : null;
      }().timeout(timeout);
    } on Object {
      return null;
    }
  }
}
