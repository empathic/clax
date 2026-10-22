import 'package:flutter/services.dart';

import 'device_ports.dart';

/// A page's shares on the Mac: an `NSSharingServicePicker` at the centre
/// of the main window, drawn by `MainFlutterWindow.swift` over the method
/// channel `dev.empathic.clax/native` (`share` {text} or {name, data},
/// answering whether the person chose a target). `share_plus`'s macOS
/// plugin is not used: it shows its picker in the engine's implicit view,
/// which no Mac window shows.
class MacShare implements SharePort {
  const MacShare();

  static const channel = MethodChannel('dev.empathic.clax/native');

  @override
  Future<bool> text(String text) => _share({'text': text});

  @override
  Future<bool> file(String name, String mime, Uint8List data) =>
      _share({'name': name, 'data': data});

  Future<bool> _share(Map<String, Object> args) async =>
      await channel.invokeMethod<bool>('share', args) ?? false;
}
