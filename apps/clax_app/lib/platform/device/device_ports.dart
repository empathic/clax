/// The device features the device capabilities reach, each behind a small
/// interface: the plugins implement them (`device_plugins.dart`), tests
/// give fakes.
library;

import 'dart:typed_data';

/// A device call's coded failure, which the page's call rejects with:
/// `not_granted`, `cancelled`, `unavailable`, `bad_request` or `too_large`
/// (`web/contract/clax-extensions.d.ts`, `Device.ErrorCode`).
class DeviceError implements Exception {
  const DeviceError(this.code, this.message);

  const DeviceError.notGranted(this.message) : code = 'not_granted';
  const DeviceError.cancelled(this.message) : code = 'cancelled';
  const DeviceError.unavailable(this.message) : code = 'unavailable';
  const DeviceError.badRequest(this.message) : code = 'bad_request';

  final String code;
  final String message;

  @override
  String toString() => 'DeviceError($code: $message)';
}

/// How a reminder repeats from its first time.
enum Repeat { daily, weekly }

/// The system's notifications. Each native notification has an integer
/// [id] and carries a [payload] handed back when it is tapped.
abstract interface class NotifierPort {
  /// Sets the plugin up; [onTap] gets the payload of each notification
  /// the person taps while the app runs.
  Future<void> init(void Function(String payload) onTap);

  /// The payload of the notification whose tap launched the app, if one
  /// did.
  Future<String?> launchPayload();

  /// Asks for the system permission to post notifications; whether it is
  /// granted.
  Future<bool> requestPermission();

  /// Shows a notification at [at] (local time), then every day or week
  /// when [repeat] says so.
  Future<void> schedule({
    required int id,
    required String title,
    required String body,
    required DateTime at,
    Repeat? repeat,
    required String payload,
  });

  Future<void> cancel(int id);
}

abstract interface class SharePort {
  /// Opens the share sheet with [text]; whether the person chose a target.
  Future<bool> text(String text);

  /// Opens the share sheet with a file; whether the person chose a target.
  Future<bool> file(String name, String mime, Uint8List data);
}

/// A file the person picked.
class FileData {
  const FileData(this.name, this.mime, this.data);
  final String name;
  final String mime;
  final Uint8List data;
}

abstract interface class FilesPort {
  /// One file the person picks, of the [extensions] given (any when
  /// empty); null when they cancel.
  Future<FileData?> pick(List<String> extensions);

  /// Lets the person save a file where they choose; false when they
  /// cancel.
  Future<bool> save(String name, String mime, Uint8List data);
}

/// A photo, at most [CameraPort.maxSide] on each side.
class Photo {
  const Photo(this.mime, this.data, this.width, this.height);
  final String mime;
  final Uint8List data;
  final int width;
  final int height;
}

abstract interface class CameraPort {
  /// The longest side a photo is downscaled to.
  static const maxSide = 1600;

  /// A photo taken with the camera; null when the person cancels.
  /// Throws [DeviceError] `unavailable` where there is no camera.
  Future<Photo?> take();

  /// A photo from the person's library; null when they cancel.
  Future<Photo?> pick();
}

abstract interface class LocationPort {
  /// Where the device is now. Throws [DeviceError] `not_granted` when the
  /// system permission is refused, `unavailable` when location is off or
  /// does not answer.
  Future<({double lat, double lon, double accuracy})> current();
}
