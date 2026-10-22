import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter_local_notifications/flutter_local_notifications.dart';

/// The app's one `flutter_local_notifications` plugin. iOS allows one
/// notification delegate, so every notification the app posts goes through
/// this hub: reminders (kind `reminder`) and, on iOS, how a run ended or
/// paused (kind `run`).
///
/// A notification's payload is `<kind>:<data>`, `<kind>` lowercase letters.
/// A tap is dispatched to [taps] of its kind with its `<data>`. A payload
/// that is not of that form is a reminder scheduled before the hub, whose
/// payload was the reminder's JSON alone: it is a `reminder` tap with the
/// whole payload as its data.
///
/// The plugin is initialised once, on first use ([ready]), asking for no
/// permission; [requestPermission] asks.
class NotificationHub {
  /// A hub over [plugin] (the app's one plugin when null), initialised on
  /// first use.
  NotificationHub({FlutterLocalNotificationsPlugin? plugin})
    : plugin = plugin ?? FlutterLocalNotificationsPlugin();

  /// A hub over [plugin], initialised.
  static Future<NotificationHub> init({
    FlutterLocalNotificationsPlugin? plugin,
  }) async {
    final hub = NotificationHub(plugin: plugin);
    await hub.ready;
    return hub;
  }

  /// The plugin, for what the hub does not wrap (a reminder's schedule).
  /// Its payloads must come from [payload].
  final FlutterLocalNotificationsPlugin plugin;

  final _taps = StreamController<(String, String)>.broadcast();
  Future<void>? _ready;
  Future<(String, String)?>? _launch;
  final _launchTaken = <String>{};

  /// The payload of a notification of [kind] carrying [data].
  static String payload(String kind, String data) => '$kind:$data';

  static final _kinded = RegExp(r'^([a-z]+):');

  /// [payload]'s kind and data.
  static (String, String) parse(String payload) {
    final m = _kinded.firstMatch(payload);
    if (m == null) return ('reminder', payload);
    return (m[1]!, payload.substring(m.end));
  }

  /// Completes once the plugin is initialised.
  Future<void> get ready => _ready ??= _initialize();

  Future<void> _initialize() async {
    const darwin = DarwinInitializationSettings(
      requestAlertPermission: false,
      requestSoundPermission: false,
      requestBadgePermission: false,
    );
    await plugin.initialize(
      settings: const InitializationSettings(
        android: AndroidInitializationSettings('@mipmap/ic_launcher'),
        iOS: darwin,
        macOS: darwin,
      ),
      onDidReceiveNotificationResponse: (r) {
        if (r.payload case final p?) _taps.add(parse(p));
      },
    );
  }

  /// The data of taps on notifications of [kind] while the app runs.
  Stream<String> taps(String kind) {
    unawaited(ready);
    return _taps.stream.where((t) => t.$1 == kind).map((t) => t.$2);
  }

  /// The data of the notification of [kind] that launched the app, the
  /// first time it is asked for; null after, and for any other launch.
  Future<String?> launch(String kind) async {
    final launched = await (_launch ??= _launchPayload());
    if (launched == null || launched.$1 != kind) return null;
    if (!_launchTaken.add(kind)) return null;
    return launched.$2;
  }

  Future<(String, String)?> _launchPayload() async {
    await ready;
    final d = await plugin.getNotificationAppLaunchDetails();
    if (d == null || !d.didNotificationLaunchApp) return null;
    final p = d.notificationResponse?.payload;
    return p == null ? null : parse(p);
  }

  /// Asks for the system permission to post notifications (Android 13 and
  /// later, iOS, macOS); whether it is granted. Elsewhere false.
  Future<bool> requestPermission() async {
    await ready;
    switch (defaultTargetPlatform) {
      case TargetPlatform.android:
        final a = plugin
            .resolvePlatformSpecificImplementation<
              AndroidFlutterLocalNotificationsPlugin
            >();
        return await a?.requestNotificationsPermission() ?? false;
      case TargetPlatform.iOS:
        final i = plugin
            .resolvePlatformSpecificImplementation<
              IOSFlutterLocalNotificationsPlugin
            >();
        return await i?.requestPermissions(alert: true, sound: true) ?? false;
      case TargetPlatform.macOS:
        final m = plugin
            .resolvePlatformSpecificImplementation<
              MacOSFlutterLocalNotificationsPlugin
            >();
        return await m?.requestPermissions(alert: true, sound: true) ?? false;
      default:
        return false;
    }
  }

  /// Shows notification [id] now, replacing one already shown with that
  /// ID; tapping it reaches [taps] of [kind] with [data]. iOS and macOS
  /// only: Android posts its run notices from `BackgroundRunService.kt`.
  Future<void> show(
    int id,
    String title,
    String body, {
    required String kind,
    required String data,
  }) async {
    await ready;
    await plugin.show(
      id: id,
      title: title,
      body: body.isEmpty ? null : body,
      notificationDetails: const NotificationDetails(
        iOS: DarwinNotificationDetails(),
        macOS: DarwinNotificationDetails(),
      ),
      payload: payload(kind, data),
    );
  }

  /// Removes notification [id], shown or pending.
  Future<void> cancel(int id) async {
    await ready;
    await plugin.cancel(id: id);
  }
}
