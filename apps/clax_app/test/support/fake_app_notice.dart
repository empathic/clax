import 'dart:async';

import 'package:clax_app/platform/app_notice.dart';

/// Records every [AppNotice] call as `show(aid, title)`, `clear`,
/// `allowed` or `turnOn`, answers [allowed] with [allow] and [turnOn] with
/// [turnOnAnswer] (which, when allowed, allows), and plays taps.
class FakeAppNotice implements AppNotice {
  FakeAppNotice({this.allow = true});

  bool allow;
  NoticeTurnOn turnOnAnswer = NoticeTurnOn.allowed;
  final calls = <String>[];
  AppNoticeTap? launch;
  final _taps = StreamController<AppNoticeTap>.broadcast(sync: true);

  /// The notice the platform would show now: `aid title`, or null.
  String? shown;

  final _dismissed = StreamController<String>.broadcast(sync: true);

  /// The person swiped the notice away.
  void dismiss() {
    final aid = shown?.split(' ').first;
    shown = null;
    if (aid != null) _dismissed.add(aid);
  }

  @override
  Stream<String> get dismissed => _dismissed.stream;

  void tap(String aid, AppAction action) =>
      _taps.add(AppNoticeTap(aid, action));

  @override
  Future<void> show(String artifactId, String title) async {
    calls.add('show($artifactId, $title)');
    shown = '$artifactId $title';
  }

  @override
  Future<void> clear() async {
    calls.add('clear');
    shown = null;
  }

  @override
  Future<bool> allowed() async {
    calls.add('allowed');
    return allow;
  }

  @override
  Future<NoticeTurnOn> turnOn() async {
    calls.add('turnOn');
    if (turnOnAnswer == NoticeTurnOn.allowed) allow = true;
    return turnOnAnswer;
  }

  @override
  Stream<AppNoticeTap> get taps => _taps.stream;

  @override
  Future<AppNoticeTap?> launchTap() async => launch;
}
