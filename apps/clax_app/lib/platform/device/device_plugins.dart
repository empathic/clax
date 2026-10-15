/// The device ports over their plugins: flutter_local_notifications (with
/// timezone), share_plus, file_picker, image_picker and geolocator. Where a
/// platform lacks a feature the port throws [DeviceError] `unavailable`.
library;

import 'dart:async';
import 'dart:ui' as ui;

import 'package:file_picker/file_picker.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';
import 'package:flutter_timezone/flutter_timezone.dart';
import 'package:geolocator/geolocator.dart';
import 'package:image_picker/image_picker.dart';
import 'package:share_plus/share_plus.dart';
import 'package:timezone/data/latest.dart' as tzdata;
import 'package:timezone/timezone.dart' as tz;

import 'device_ports.dart';

bool get _android => defaultTargetPlatform == TargetPlatform.android;
bool get _macOS => defaultTargetPlatform == TargetPlatform.macOS;

/// Notifications through flutter_local_notifications: the app's reminders
/// channel on Android (inexact alarms allowed while idle, so no exact-alarm
/// permission is needed), the notification center on macOS.
class PluginNotifier implements NotifierPort {
  final _plugin = FlutterLocalNotificationsPlugin();
  static const _channel = AndroidNotificationDetails(
    'reminders',
    'Reminders',
    channelDescription: 'Reminders your apps schedule',
    importance: Importance.high,
    priority: Priority.high,
  );

  @override
  Future<void> init(void Function(String payload) onTap) async {
    if (!_android && !_macOS) {
      throw const DeviceError.unavailable('no notifications on this platform');
    }
    tzdata.initializeTimeZones();
    try {
      final zone = await FlutterTimezone.getLocalTimezone();
      tz.setLocalLocation(tz.getLocation(zone.identifier));
    } on Object {
      // The zone is unknown: times are taken as UTC.
    }
    const darwin = DarwinInitializationSettings(
      requestAlertPermission: false,
      requestSoundPermission: false,
      requestBadgePermission: false,
    );
    await _plugin.initialize(
      settings: const InitializationSettings(
        android: AndroidInitializationSettings('@mipmap/ic_launcher'),
        macOS: darwin,
      ),
      onDidReceiveNotificationResponse: (r) {
        if (r.payload case final p?) onTap(p);
      },
    );
  }

  @override
  Future<String?> launchPayload() async {
    final d = await _plugin.getNotificationAppLaunchDetails();
    if (d == null || !d.didNotificationLaunchApp) return null;
    return d.notificationResponse?.payload;
  }

  @override
  Future<bool> requestPermission() async {
    if (_android) {
      final a = _plugin
          .resolvePlatformSpecificImplementation<
            AndroidFlutterLocalNotificationsPlugin
          >();
      return await a?.requestNotificationsPermission() ?? false;
    }
    if (_macOS) {
      final m = _plugin
          .resolvePlatformSpecificImplementation<
            MacOSFlutterLocalNotificationsPlugin
          >();
      return await m?.requestPermissions(alert: true, sound: true) ?? false;
    }
    return false;
  }

  @override
  Future<void> schedule({
    required int id,
    required String title,
    required String body,
    required DateTime at,
    Repeat? repeat,
    required String payload,
  }) => _plugin.zonedSchedule(
    id: id,
    title: title,
    body: body.isEmpty ? null : body,
    scheduledDate: tz.TZDateTime.from(at, tz.local),
    notificationDetails: const NotificationDetails(
      android: _channel,
      macOS: DarwinNotificationDetails(),
    ),
    androidScheduleMode: AndroidScheduleMode.inexactAllowWhileIdle,
    matchDateTimeComponents: switch (repeat) {
      null => null,
      Repeat.daily => DateTimeComponents.time,
      Repeat.weekly => DateTimeComponents.dayOfWeekAndTime,
    },
    payload: payload,
  );

  @override
  Future<void> cancel(int id) => _plugin.cancel(id: id);
}

class PluginShare implements SharePort {
  static bool _done(ShareResult r) => r.status != ShareResultStatus.dismissed;

  @override
  Future<bool> text(String text) async =>
      _done(await SharePlus.instance.share(ShareParams(text: text)));

  @override
  Future<bool> file(String name, String mime, Uint8List data) async => _done(
    await SharePlus.instance.share(
      ShareParams(
        files: [XFile.fromData(data, mimeType: mime, name: name)],
        fileNameOverrides: [name],
      ),
    ),
  );
}

class PluginFiles implements FilesPort {
  @override
  Future<FileData?> pick(List<String> extensions) async {
    final f = await FilePicker.pickFile(
      type: extensions.isEmpty ? FileType.any : FileType.custom,
      allowedExtensions: extensions.isEmpty ? null : extensions,
    );
    if (f == null) return null;
    final ext = (f.extension ?? '').toLowerCase();
    return FileData(f.name, _mimeOf(ext), await f.readAsBytes());
  }

  @override
  Future<bool> save(String name, String mime, Uint8List data) async =>
      await FilePicker.saveFile(fileName: name, bytes: data, mimeType: mime) !=
      null;
}

String _mimeOf(String ext) => switch (ext) {
  'csv' => 'text/csv',
  'txt' => 'text/plain',
  'md' => 'text/markdown',
  'json' => 'application/json',
  'pdf' => 'application/pdf',
  'png' => 'image/png',
  'jpg' || 'jpeg' => 'image/jpeg',
  'gif' => 'image/gif',
  'webp' => 'image/webp',
  'heic' => 'image/heic',
  _ => 'application/octet-stream',
};

/// Photos through image_picker, downscaled by the platform to at most
/// [CameraPort.maxSide] on each side. macOS has no camera source.
class PluginCamera implements CameraPort {
  final _picker = ImagePicker();

  Future<Photo?> _get(ImageSource source) async {
    if (!_picker.supportsImageSource(source)) {
      throw const DeviceError.unavailable(
        'this device has no camera to take photos with',
      );
    }
    final x = await _picker.pickImage(
      source: source,
      maxWidth: CameraPort.maxSide.toDouble(),
      maxHeight: CameraPort.maxSide.toDouble(),
      imageQuality: 85,
    );
    if (x == null) return null;
    final bytes = await x.readAsBytes();
    final (w, h) = await _size(bytes);
    final ext = x.name.split('.').last.toLowerCase();
    return Photo(x.mimeType ?? _mimeOf(ext), bytes, w, h);
  }

  static Future<(int, int)> _size(Uint8List bytes) async {
    final buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
    final d = await ui.ImageDescriptor.encoded(buffer);
    final size = (d.width, d.height);
    d.dispose();
    buffer.dispose();
    return size;
  }

  @override
  Future<Photo?> take() => _get(ImageSource.camera);

  @override
  Future<Photo?> pick() => _get(ImageSource.gallery);
}

/// Location through geolocator, on Android only. On macOS every call is
/// `unavailable`: Mac location needs a usage description, the location
/// entitlement and a signed build, which Clax does not have yet.
///
/// Precise asks for high accuracy, Approximate for low power (Android's
/// coarse location). A fix is precise only when it was asked precise and
/// Android lets Clax have a precise location (the person may grant
/// approximate only).
class PluginLocation implements LocationPort {
  /// Checks the platform, the device's location and Clax's permission,
  /// asking for it when Android still may.
  Future<void> _ready() async {
    if (!_android) {
      throw DeviceError.unavailable(
        _macOS
            ? 'location is not available in Clax on macOS yet; it works in '
                  'Clax on Android'
            : 'location is not available on this platform',
      );
    }
    if (!await Geolocator.isLocationServiceEnabled()) {
      throw const DeviceError(
        'location_off',
        'location is off on this device; turn it on in settings',
      );
    }
    var p = await Geolocator.checkPermission();
    if (p == LocationPermission.denied) {
      p = await Geolocator.requestPermission();
    }
    if (p == LocationPermission.deniedForever) {
      throw const DeviceError(
        'system_denied',
        'location is off for Clax in system settings, and Android will not '
            'ask again',
      );
    }
    if (p == LocationPermission.denied ||
        p == LocationPermission.unableToDetermine) {
      throw const DeviceError.notGranted('location is not allowed for Clax');
    }
  }

  /// Whether a fix asked at [precision] is precise.
  static Future<bool> _precise(Precision precision) async =>
      precision == Precision.precise &&
      await Geolocator.getLocationAccuracy() == LocationAccuracyStatus.precise;

  static LocationAccuracy _accuracy(Precision p) =>
      p == Precision.precise ? LocationAccuracy.high : LocationAccuracy.low;

  @override
  Future<LocationFix> current(Precision precision) async {
    await _ready();
    final precise = await _precise(precision);
    try {
      final pos = await Geolocator.getCurrentPosition(
        locationSettings: LocationSettings(
          accuracy: _accuracy(precision),
          timeLimit: const Duration(seconds: 20),
        ),
      );
      return LocationFix(
        pos.latitude,
        pos.longitude,
        pos.accuracy,
        precise: precise,
      );
    } on TimeoutException {
      throw const DeviceError.unavailable(
        'the device did not find its location in time',
      );
    }
  }

  @override
  Future<Stream<LocationFix>> watch(
    Precision precision, {
    required double minDistance,
  }) async {
    await _ready();
    final precise = await _precise(precision);
    return Geolocator.getPositionStream(
      locationSettings: AndroidSettings(
        accuracy: _accuracy(precision),
        distanceFilter: minDistance.round(),
        intervalDuration: const Duration(seconds: 1),
      ),
    ).transform(
      StreamTransformer.fromHandlers(
        handleData: (pos, sink) => sink.add(
          LocationFix(
            pos.latitude,
            pos.longitude,
            pos.accuracy,
            precise: precise,
          ),
        ),
        handleError: (e, st, sink) => sink.addError(switch (e) {
          LocationServiceDisabledException() => const DeviceError(
            'location_off',
            'location was turned off on this device',
          ),
          PermissionDeniedException() => const DeviceError.notGranted(
            'location is no longer allowed for Clax',
          ),
          _ => DeviceError.unavailable('$e'),
        }),
      ),
    );
  }

  @override
  Future<void> openSettings(LocationSettingsPage page) async {
    if (!_android) return;
    switch (page) {
      case LocationSettingsPage.app:
        await Geolocator.openAppSettings();
      case LocationSettingsPage.location:
        await Geolocator.openLocationSettings();
    }
  }
}
