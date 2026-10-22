import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// What a tap on the open app's notice asks for.
enum AppAction {
  /// The notice's body: Clax in front, on that app.
  open,

  /// "Comment": comment mode on that app.
  comment,

  /// "Threads": that app's threads panel.
  threads,
}

/// A tap on the notice of app [artifact].
@immutable
final class AppNoticeTap {
  const AppNoticeTap(this.artifact, this.action);

  final String artifact;
  final AppAction action;

  @override
  bool operator ==(Object other) =>
      other is AppNoticeTap &&
      other.artifact == artifact &&
      other.action == action;

  @override
  int get hashCode => Object.hash(artifact, action);

  @override
  String toString() => 'AppNoticeTap($artifact, ${action.name})';
}

/// The platform's notice for the app on screen: on Android an ongoing,
/// silent notification naming the app, with "Comment" and "Threads"
/// actions, which is how the person reaches Clax's comment tools without
/// any Clax control drawn over the app. Every method is best effort: none
/// throws.
abstract interface class AppNotice {
  /// Shows the notice for app [artifactId], called [title], replacing any
  /// other.
  Future<void> show(String artifactId, String title);

  /// Removes the notice.
  Future<void> clear();

  /// Whether the notice can be seen: notifications are allowed for Clax
  /// and for the notice's channel.
  Future<bool> allowed();

  /// Taps on the notice while Clax runs.
  Stream<AppNoticeTap> get taps;

  /// The artifact IDs of notices the person swiped away while Clax runs.
  Stream<String> get dismissed;

  /// The tap that launched Clax, once; null for any other launch.
  Future<AppNoticeTap?> launchTap();
}

/// [AppNotice] over the `dev.empathic.clax/app_notice` channel, which
/// `MainActivity` serves on Android.
///
/// Dart to platform: `show` {artifact, title}, `clear`, `allowed` → bool,
/// `launchTap` → {artifact, action}?. Platform to Dart: `tapped`
/// {artifact, action}, the action `open`, `comment` or `threads`, and
/// `dismissed` {artifact} when the notice is swiped away.
class ChannelAppNotice implements AppNotice {
  ChannelAppNotice({this._channel = const MethodChannel(channelName)}) {
    _channel.setMethodCallHandler(_onCall);
  }

  static const channelName = 'dev.empathic.clax/app_notice';

  final MethodChannel _channel;
  final _taps = StreamController<AppNoticeTap>.broadcast();
  final _dismissed = StreamController<String>.broadcast();

  /// Reads a tap from the platform's {artifact, action}; null for anything
  /// else.
  static AppNoticeTap? tapOf(Object? args) => switch (args) {
    {'artifact': final String aid, 'action': final String action}
        when aid.isNotEmpty =>
      switch (AppAction.values.where((a) => a.name == action).firstOrNull) {
        final a? => AppNoticeTap(aid, a),
        null => null,
      },
    _ => null,
  };

  Future<Object?> _onCall(MethodCall call) async {
    switch (call) {
      case MethodCall(method: 'tapped', :final arguments):
        if (tapOf(arguments) case final tap?) _taps.add(tap);
      case MethodCall(
        method: 'dismissed',
        arguments: {'artifact': final String aid},
      ):
        _dismissed.add(aid);
    }
    return null;
  }

  Future<T?> _call<T>(String method, [Map<String, Object?>? args]) async {
    try {
      return await _channel.invokeMethod<T>(method, args);
    } on PlatformException {
      return null;
    } on MissingPluginException {
      return null;
    }
  }

  @override
  Future<void> show(String artifactId, String title) =>
      _call<void>('show', {'artifact': artifactId, 'title': title});

  @override
  Future<void> clear() => _call<void>('clear');

  @override
  Future<bool> allowed() async => await _call<bool>('allowed') ?? false;

  @override
  Stream<AppNoticeTap> get taps => _taps.stream;

  @override
  Stream<String> get dismissed => _dismissed.stream;

  @override
  Future<AppNoticeTap?> launchTap() async =>
      tapOf(await _call<Object?>('launchTap'));
}

/// The one notice, shared by the app screens: each screen shows it while
/// it is on screen and clears it as it leaves, and a clear from a screen
/// whose notice another has replaced since does nothing (a screen pushed
/// over another may show its notice before the one below hears it is
/// covered).
class AppNoticeSlot {
  AppNoticeSlot(this.notice);

  final AppNotice notice;
  Object? _owner;

  /// The artifact IDs of notices the person swiped away.
  Stream<String> get dismissed => notice.dismissed;

  /// Whether the notice can be seen ([AppNotice.allowed]).
  Future<bool> allowed() async {
    try {
      return await notice.allowed();
    } on Object {
      return false;
    }
  }

  /// Shows [owner]'s notice for app [artifactId], called [title].
  void show(Object owner, String artifactId, String title) {
    _owner = owner;
    unawaited(notice.show(artifactId, title));
  }

  /// Removes the notice when [owner] showed it last.
  void clear(Object owner) {
    if (!identical(_owner, owner)) return;
    _owner = null;
    unawaited(notice.clear());
  }
}
