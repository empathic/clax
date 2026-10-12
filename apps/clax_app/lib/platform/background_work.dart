import 'dart:async';

import 'package:flutter/services.dart';

/// The platform's side of runs that go on while the person is elsewhere:
/// on Android, a `dataSync` foreground service with an ongoing progress
/// notification (which keeps the process and its network alive in the
/// background), and the notifications that say how a run ended. Every
/// method is best effort: none throws, and a platform without the service
/// answers false or null.
abstract interface class BackgroundWork {
  /// Starts the service, showing [title] and [step]; true when it runs.
  /// Android refuses a start while the app is in the background.
  Future<bool> start(String title, String step);

  /// Shows [title] and [step] on the running service's notification.
  Future<void> update(String title, String step);

  /// Stops the service and removes its notification.
  Future<void> stop();

  /// Posts "[title] is ready"-style news; tapping it opens [artifactId].
  Future<void> ready(String title, String text, String artifactId);

  /// Posts that a run failed for [reason]; tapping it opens [artifactId],
  /// or the app when null.
  Future<void> failed(String title, String reason, {String? artifactId});

  /// Asks for permission to post notifications, once per install (Android
  /// 13 and later). True when notifications may be posted.
  Future<bool> requestPermission();

  /// The artifact ID of the notification that launched the app, once; null
  /// for any other launch.
  Future<String?> launchArtifact();

  /// Artifact IDs of notifications tapped while the app runs.
  Stream<String> get opened;

  /// An event each time the service stopped on its own: its start was
  /// refused, the system's time limit for it was spent, or the app's task
  /// was removed.
  Stream<void> get stopped;
}

/// [BackgroundWork] where there is none (macOS, tests): nothing runs and
/// nothing is posted.
class NoBackgroundWork implements BackgroundWork {
  const NoBackgroundWork();

  @override
  Future<bool> start(String title, String step) async => false;
  @override
  Future<void> update(String title, String step) async {}
  @override
  Future<void> stop() async {}
  @override
  Future<void> ready(String title, String text, String artifactId) async {}
  @override
  Future<void> failed(
    String title,
    String reason, {
    String? artifactId,
  }) async {}
  @override
  Future<bool> requestPermission() async => false;
  @override
  Future<String?> launchArtifact() async => null;
  @override
  Stream<String> get opened => const Stream.empty();
  @override
  Stream<void> get stopped => const Stream.empty();
}

/// [BackgroundWork] over the `dev.empathic.clax/background` channel, which
/// `MainActivity` serves on Android.
///
/// Dart to platform: `start` {title, text} → bool, `update` {title, text},
/// `stop`, `ready` {title, text, artifact}, `failed` {title, text,
/// artifact?}, `requestPermission` → bool, `launchArtifact` → String?.
/// Platform to Dart: `opened` {artifact} when a notification is tapped
/// while the app runs, and `stopped` when the service stopped on its own.
class ChannelBackgroundWork implements BackgroundWork {
  ChannelBackgroundWork({this._channel = const MethodChannel(channelName)}) {
    _channel.setMethodCallHandler(_onCall);
  }

  static const channelName = 'dev.empathic.clax/background';

  final MethodChannel _channel;
  final _opened = StreamController<String>.broadcast();
  final _stoppedEvents = StreamController<void>.broadcast();

  Future<Object?> _onCall(MethodCall call) async {
    switch (call) {
      case MethodCall(
        method: 'opened',
        arguments: {'artifact': final String aid},
      ):
        _opened.add(aid);
      case MethodCall(method: 'stopped'):
        _stoppedEvents.add(null);
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
  Future<bool> start(String title, String step) async =>
      await _call<bool>('start', {'title': title, 'text': step}) ?? false;

  @override
  Future<void> update(String title, String step) =>
      _call<void>('update', {'title': title, 'text': step});

  @override
  Future<void> stop() => _call<void>('stop');

  @override
  Future<void> ready(String title, String text, String artifactId) =>
      _call<void>('ready', {
        'title': title,
        'text': text,
        'artifact': artifactId,
      });

  @override
  Future<void> failed(String title, String reason, {String? artifactId}) =>
      _call<void>('failed', {
        'title': title,
        'text': reason,
        'artifact': artifactId,
      });

  @override
  Future<bool> requestPermission() async =>
      await _call<bool>('requestPermission') ?? false;

  @override
  Future<String?> launchArtifact() => _call<String>('launchArtifact');

  @override
  Stream<String> get opened => _opened.stream;

  @override
  Stream<void> get stopped => _stoppedEvents.stream;
}

/// One run [BackgroundRuns.begin] is showing.
final class BackgroundRun {
  BackgroundRun._(this.title, this.step);

  /// What the notification calls the run ("Making “a run tracker”…").
  String title;

  /// What the run is doing now.
  String step;
  bool _ended = false;
}

/// How a run ended, for [BackgroundRuns.end].
sealed class RunEnd {
  const RunEnd();
}

/// The run made or changed [artifactId]; [title] and [text] are the news.
final class RunReady extends RunEnd {
  const RunReady(this.artifactId, this.title, this.text);

  final String artifactId;
  final String title;
  final String text;
}

/// The run failed for [reason].
final class RunFailed extends RunEnd {
  const RunFailed(this.title, this.reason, {this.artifactId});

  final String title;
  final String reason;
  final String? artifactId;
}

/// The run ended with nothing to tell (cancelled, interrupted, or a reply
/// without a new version).
final class RunQuiet extends RunEnd {
  const RunQuiet();
}

/// Keeps [BackgroundWork]'s one service running while any run is: the
/// first [begin] starts it, each step updates it with the newest run's
/// title and step, and the last [end] stops it. A run that ends ready or
/// failed while the app is not in the foreground ([foreground] false) posts
/// that news; in the foreground the screen already shows it.
///
/// The service may not be running while runs are: its start was refused
/// (Clax was in the background), or it stopped on its own
/// ([BackgroundWork.stopped]). The next [begin], or [onForeground] while
/// runs go on, starts it again.
///
/// Progress hook: a run's step text comes from [step]; any source of steps
/// (the making screen's stages, the agent's streamed steps) calls it with
/// the run's handle.
class BackgroundRuns {
  BackgroundRuns(this._work, {required this._foreground}) {
    _stopped = _work.stopped.listen((_) => _down());
  }

  final BackgroundWork _work;
  final bool Function() _foreground;
  final _active = <BackgroundRun>[];
  late final StreamSubscription<void> _stopped;

  /// Whether the service was asked to run and has not been refused or
  /// stopped since.
  var _service = false;

  /// Bumped at every start and stop, so a late answer to an older start
  /// changes nothing.
  var _generation = 0;

  /// Whether any run is going.
  bool get busy => _active.isNotEmpty;

  /// Starts showing a run called [title], doing [step].
  BackgroundRun begin(String title, String step) {
    final run = BackgroundRun._(title, step);
    _active.add(run);
    if (_service) {
      unawaited(_work.update(title, step));
    } else {
      _start(run);
    }
    return run;
  }

  /// Shows that [run] is now doing [text], and is called [title] when
  /// given; ignored once it ended.
  void step(BackgroundRun run, String text, {String? title}) {
    if (run._ended || run.step == text && (title ?? run.title) == run.title) {
      return;
    }
    run.step = text;
    run.title = title ?? run.title;
    if (_service && identical(_active.last, run)) {
      unawaited(_work.update(run.title, text));
    }
  }

  /// Ends [run] as [how]; ignored when it already ended.
  void end(BackgroundRun run, RunEnd how) {
    if (run._ended) return;
    run._ended = true;
    final wasShown = identical(_active.last, run);
    _active.remove(run);
    if (_active.isEmpty) {
      _down();
      unawaited(_work.stop());
    } else if (wasShown && _service) {
      unawaited(_work.update(_active.last.title, _active.last.step));
    }
    if (_foreground()) return;
    switch (how) {
      case RunReady(:final artifactId, :final title, :final text):
        unawaited(_work.ready(title, text, artifactId));
      case RunFailed(:final title, :final reason, :final artifactId):
        unawaited(_work.failed(title, reason, artifactId: artifactId));
      case RunQuiet():
        break;
    }
  }

  /// Clax came to the foreground: starts the service again when runs go
  /// on without it.
  void onForeground() {
    if (busy && !_service) _start(_active.last);
  }

  /// Stops following the service's stops.
  void dispose() => unawaited(_stopped.cancel());

  void _start(BackgroundRun run) {
    _service = true;
    final generation = ++_generation;
    unawaited(
      _work.start(run.title, run.step).then((started) {
        if (!started && generation == _generation) _service = false;
      }),
    );
  }

  void _down() {
    _service = false;
    _generation++;
  }
}
