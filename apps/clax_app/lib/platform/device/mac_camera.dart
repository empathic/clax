import 'package:flutter/services.dart';

import 'device_ports.dart';

/// A photo taken on the Mac: the capture sheet of `Camera.swift`, on the
/// key window, over the method channel `dev.empathic.clax/camera`. `take`
/// {maxSide} answers {data, width, height} (a JPEG at most `maxSide` a
/// side), nil when the person cancels or closes the sheet, or fails with
/// `unavailable` (no camera, the camera would not start, a sheet already
/// up) or `not_granted` (macOS does not allow Clax the camera).
/// image_picker has no camera source on macOS.
class MacCamera {
  const MacCamera();

  static const channel = MethodChannel('dev.empathic.clax/camera');

  /// A photo from the capture sheet; null when the person cancels.
  Future<Photo?> take() async {
    final Object? r;
    try {
      r = await channel.invokeMethod<Object?>('take', {
        'maxSide': CameraPort.maxSide,
      });
    } on PlatformException catch (e) {
      final message = e.message ?? 'the camera did not take a photo';
      throw e.code == 'not_granted'
          ? DeviceError.notGranted(message)
          : DeviceError.unavailable(message);
    } on MissingPluginException {
      throw const DeviceError.unavailable(
        'this device has no camera to take photos with',
      );
    }
    return switch (r) {
      null => null,
      {
        'data': final Uint8List data,
        'width': final int width,
        'height': final int height,
      } =>
        Photo('image/jpeg', data, width, height),
      _ => throw const DeviceError.unavailable(
        'the camera gave no photo Clax can read',
      ),
    };
  }
}
