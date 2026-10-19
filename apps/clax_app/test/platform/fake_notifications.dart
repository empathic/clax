import 'package:flutter_local_notifications/flutter_local_notifications.dart';

/// A [FlutterLocalNotificationsPlugin] that records what the hub asks of it
/// and lets a test tap a notification or say which one launched the app.
class FakeNotificationsPlugin implements FlutterLocalNotificationsPlugin {
  var initializes = 0;
  final shown = <String>[];
  final cancelled = <int>[];
  DidReceiveNotificationResponseCallback? _onTap;

  /// The payload of the notification that launched the app, or null.
  String? launchedBy;

  /// Taps the notification carrying [payload] while the app runs.
  void tap(String payload) => _onTap!(
    NotificationResponse(
      notificationResponseType: NotificationResponseType.selectedNotification,
      payload: payload,
    ),
  );

  @override
  Future<bool?> initialize({
    required InitializationSettings settings,
    DidReceiveNotificationResponseCallback? onDidReceiveNotificationResponse,
    DidReceiveBackgroundNotificationResponseCallback?
    onDidReceiveBackgroundNotificationResponse,
  }) async {
    initializes++;
    _onTap = onDidReceiveNotificationResponse;
    return true;
  }

  @override
  Future<NotificationAppLaunchDetails?>
  getNotificationAppLaunchDetails() async => NotificationAppLaunchDetails(
    launchedBy != null,
    notificationResponse: launchedBy == null
        ? null
        : NotificationResponse(
            notificationResponseType:
                NotificationResponseType.selectedNotification,
            payload: launchedBy,
          ),
  );

  @override
  Future<void> show({
    required int id,
    String? title,
    String? body,
    NotificationDetails? notificationDetails,
    String? payload,
  }) async => shown.add('$id|$title|$body|$payload');

  @override
  Future<void> cancel({required int id, String? tag}) async =>
      cancelled.add(id);

  @override
  T? resolvePlatformSpecificImplementation<
    T extends FlutterLocalNotificationsPlatform
  >() => null;

  @override
  dynamic noSuchMethod(Invocation invocation) => super.noSuchMethod(invocation);
}
