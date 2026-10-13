import 'dart:async';

import 'package:clax_app/platform/app_notice.dart';

/// Records every [AppNotice] call as `show(aid, title)`, `clear` or
/// `allowed`, answers [allowed] with [allow], and plays taps.
class FakeAppNotice implements AppNotice {
  FakeAppNotice({this.allow = true});

  bool allow;
  final calls = <String>[];
  AppNoticeTap? launch;
  final _taps = StreamController<AppNoticeTap>.broadcast(sync: true);

  /// The notice the platform would show now: `aid title`, or null.
  String? shown;

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
  Stream<AppNoticeTap> get taps => _taps.stream;

  @override
  Future<AppNoticeTap?> launchTap() async => launch;
}
