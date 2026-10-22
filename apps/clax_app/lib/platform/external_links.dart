import 'package:flutter/services.dart';

/// Opens links the app view does not load (a web or mail link off the
/// device) in the platform's own handler: an `ACTION_VIEW` intent on
/// Android, `NSWorkspace` on macOS, both through the `openExternal` method
/// of the `dev.empathic.clax/native` channel.
///
/// Only `http`, `https` and `mailto` links are passed on; anything else is
/// refused here, whatever the caller checked.
class ExternalLinks {
  const ExternalLinks({
    this._channel = const MethodChannel('dev.empathic.clax/native'),
  });

  final MethodChannel _channel;

  /// The schemes [open] passes on.
  static const schemes = {'http', 'https', 'mailto'};

  /// Asks the platform to open [url]; true when it did. Never throws.
  Future<bool> open(Uri url) async {
    if (!schemes.contains(url.scheme.toLowerCase())) return false;
    try {
      return await _channel.invokeMethod<bool>('openExternal', {
            'url': url.toString(),
          }) ??
          false;
    } on PlatformException {
      return false;
    } on MissingPluginException {
      return false;
    }
  }
}
