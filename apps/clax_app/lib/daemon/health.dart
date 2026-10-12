import 'dart:async';
import 'dart:io';

/// Asks whether a daemon answers.
abstract interface class HealthProbe {
  /// Whether `GET <base>/healthz` answers 200.
  Future<bool> up(Uri base);
}

/// [HealthProbe] over HTTP. One probe gives up after [timeout], so a
/// wedged daemon reads as down rather than stalling the supervisor.
class HttpHealthProbe implements HealthProbe {
  HttpHealthProbe({this.timeout = const Duration(seconds: 2)})
    : _client = HttpClient()..connectionTimeout = const Duration(seconds: 1);

  final Duration timeout;
  final HttpClient _client;

  @override
  Future<bool> up(Uri base) async {
    try {
      return await () async {
        final req = await _client.getUrl(base.resolve('/healthz'));
        final res = await req.close();
        await res.drain<void>();
        return res.statusCode == 200;
      }().timeout(timeout);
    } on Object {
      return false;
    }
  }
}
