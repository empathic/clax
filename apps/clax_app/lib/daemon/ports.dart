import 'dart:io';

/// Chooses the loopback port to ask the daemon for.
abstract interface class PortPicker {
  /// [preferred] when it can be bound on loopback now, else a free port.
  Future<int> pick({int? preferred});
}

/// [PortPicker] that test-binds `127.0.0.1` and closes the socket at once.
class LoopbackPortPicker implements PortPicker {
  const LoopbackPortPicker();

  @override
  Future<int> pick({int? preferred}) async {
    if (preferred != null && preferred > 0 && preferred < 65536) {
      try {
        final s = await ServerSocket.bind(
          InternetAddress.loopbackIPv4,
          preferred,
        );
        await s.close();
        return preferred;
      } on SocketException {
        // Taken: fall through to a free port.
      }
    }
    final s = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = s.port;
    await s.close();
    return port;
  }
}
