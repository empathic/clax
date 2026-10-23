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
    return _invoke('openExternal', {'url': url.toString()});
  }

  /// Opens a sign-in page at [url] (an `https` address) where the person
  /// signs in and approves this device: in a browser session over the app
  /// where the platform has one (`ASWebAuthenticationSession` on iOS, a
  /// Custom Tab on Android), else in the browser (macOS). Sent as
  /// `openExternal` {url, mode: "auth"}; true when it opened. Never
  /// throws.
  Future<bool> openAuth(Uri url) async {
    if (url.scheme.toLowerCase() != 'https') return false;
    return _invoke('openExternal', {'url': url.toString(), 'mode': 'auth'});
  }

  /// Closes the sign-in session [openAuth] opened over the app, once it is
  /// done with (iOS; elsewhere the person closes the browser). Never
  /// throws.
  Future<void> closeAuth() => _invoke('closeAuth', null);

  /// What this device is called (`deviceName` on the channel: the
  /// device's name on Android and the Mac, its model on iOS), or null
  /// where the platform does not say. Never throws.
  Future<String?> deviceName() async {
    try {
      final n = await _channel.invokeMethod<String>('deviceName');
      return n == null || n.trim().isEmpty ? null : n.trim();
    } on PlatformException {
      return null;
    } on MissingPluginException {
      return null;
    }
  }

  Future<bool> _invoke(String method, Object? args) async {
    try {
      return await _channel.invokeMethod<bool>(method, args) ?? false;
    } on PlatformException {
      return false;
    } on MissingPluginException {
      return false;
    }
  }
}
