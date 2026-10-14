/// The in-app eyes of the `render` tool: one WebView that loads a draft in
/// the shell's inspect view and reports what it shows.
library;

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../agent/inspector.dart';
import '../daemon/clock.dart';
import 'app_surface.dart';
import 'host_protocol.dart';

/// An [Inspector] over one page surface (a WebView) that [InspectorHost]
/// keeps laid out, under the app, for the app's life.
///
/// Each inspection:
///
/// 1. tells the surface it is shown (`setVisible(true)`: a hidden WebView
///    document stops `requestAnimationFrame`), then loads the URL, a new
///    document whose nonce the surface hands the shell with `init`;
/// 2. waits for the shell's `ready` of the URL's artifact, at most
///    [readyLimit];
/// 3. calls `claxHost.inspect({settleMs, maxBytes})` and waits for its
///    `inspection`, at most the settle time plus [answerLimit];
/// 4. tells the surface it is hidden again, so the draft's page idles
///    until the next inspection replaces it.
///
/// A test run ([runTests]) goes the same way with `claxHost.runTests`, and
/// a migration run ([runMigration]) with `claxHost.runMigration`.
/// Inspections and test runs go one at a time, in the order asked; a later
/// one waits for the earlier one to end. A cancel ends the call at once with
/// [InspectorError] `cancelled`. The surface is made on the first
/// inspection, for the daemon's address the URL names, and made again when
/// that address changes (a daemon restarted on a new port).
class WebViewInspector implements Inspector {
  WebViewInspector({
    required AppSurface Function(Uri base) surface,
    this._clock = const SystemClock(),
    this.readyLimit = const Duration(seconds: 10),
    this.answerLimit = const Duration(seconds: 10),
  }) : _make = surface;

  final AppSurface Function(Uri base) _make;
  final Clock _clock;

  /// How long a load may take to greet (`ready`).
  final Duration readyLimit;

  /// How long the page may take to answer `inspect`, beyond its settle
  /// time.
  final Duration answerLimit;

  final _surface = ValueNotifier<AppSurface?>(null);
  Uri? _base;
  Future<void> _last = Future.value();
  var _disposed = false;

  /// The surface [InspectorHost] shows; null before the first inspection.
  ValueListenable<AppSurface?> get surface => _surface;

  @override
  Future<Inspection> inspect(
    Uri url, {
    Duration settle = const Duration(seconds: 3),
    int maxBytes = 1 << 20,
    Future<void>? cancelled,
  }) => _queued(cancelled, () async {
    final (m, took) = await _ask<InspectionMsg>(
      url,
      Inspect(settleMs: settle.inMilliseconds, maxBytes: maxBytes),
      settle + answerLimit,
      cancelled,
    );
    if (m.error case final why?) throw InspectorError(why);
    return Inspection(
      png: m.png,
      errors: m.errors,
      failed: m.failed,
      text: m.text,
      title: m.title,
      height: m.height,
      settledMs: took.inMilliseconds,
    );
  });

  /// Each test run loads [url] as an inspection does (steps 1, 2 and 4),
  /// then calls `claxHost.runTests` and waits for its `tests`, at most
  /// [total] plus [answerLimit].
  @override
  Future<TestRun> runTests(
    Uri url, {
    required List<String> files,
    Duration perTest = const Duration(seconds: 5),
    Duration total = const Duration(seconds: 30),
    Future<void>? cancelled,
  }) => _queued(cancelled, () async {
    final (m, _) = await _ask<TestsMsg>(
      url,
      RunTests(
        files,
        testMs: perTest.inMilliseconds,
        totalMs: total.inMilliseconds,
      ),
      total + answerLimit,
      cancelled,
    );
    if (m.error case final why?) throw InspectorError(why);
    return TestRun(passed: m.passed, failed: m.failed, skipped: m.skipped);
  });

  /// Each migration run loads [url] in the migration view (its query
  /// gains `migrate=1`: the page's bridge ends the page's markup before any
  /// of it runs, so only the migration writes), as an inspection does
  /// (steps 1, 2 and 4), then calls `claxHost.runMigration` and waits for
  /// its `migration`, at most [total] plus [answerLimit].
  ///
  /// Whatever the outcome (an answer, a timeout, a cancel), it then loads
  /// the migration view again and waits for its `ready`, at most
  /// [readyLimit], before it returns: the document the migration ran in is
  /// gone by then, so none of its code is still writing.
  @override
  Future<MigrationRun> runMigration(
    Uri url, {
    String file = 'migrate.js',
    required int from,
    required int to,
    Duration total = const Duration(seconds: 30),
    Future<void>? cancelled,
  }) => _queued(cancelled, () async {
    final view = url.replace(
      queryParameters: {...url.queryParameters, 'migrate': '1'},
    );
    try {
      final (m, _) = await _ask<MigrationMsg>(
        view,
        RunMigration(
          file: file,
          from: from,
          to: to,
          totalMs: total.inMilliseconds,
        ),
        total + answerLimit,
        cancelled,
      );
      return m.ok
          ? const MigrationRun.passed()
          : MigrationRun.failed(m.error ?? 'the migration failed');
    } finally {
      await _replace(view);
    }
  });

  /// Loads [url] again in place of the document the surface shows, and
  /// waits for its `ready`, at most [readyLimit]; a failure or no `ready`
  /// in time is let go (the load was still asked for).
  Future<void> _replace(Uri url) async {
    final surface = _surface.value;
    if (_disposed || surface == null) return;
    final aid = _artifactOf(url);
    final ready = Completer<void>();
    final sub = surface.messages.listen((m) {
      if (m is Ready && m.artifact == aid && !ready.isCompleted) {
        ready.complete();
      }
    });
    try {
      surface.setVisible(true);
      await surface.load(url);
      await Future.any<void>([ready.future, _clock.delay(readyLimit)]);
    } on Object {
      // The old document is being replaced either way.
    } finally {
      unawaited(sub.cancel());
      if (!_disposed) surface.setVisible(false);
    }
  }

  /// Runs [work] after every call before it has ended; a cancel before its
  /// turn ends it with `cancelled`.
  Future<T> _queued<T>(Future<void>? cancelled, Future<T> Function() work) {
    final prior = _last;
    final done = Completer<void>();
    _last = done.future;
    var stopped = false;
    cancelled?.then((_) => stopped = true, onError: (Object _) {});
    return () async {
      try {
        await Future.any<void>([prior, ?cancelled]);
        if (_disposed) throw const InspectorError('the app is closing');
        if (stopped) throw const InspectorError('cancelled');
        return await work();
      } finally {
        // A cancelled call waiting its turn still lets the next one go only
        // after the inspection before it ends.
        unawaited(prior.whenComplete(done.complete));
      }
    }();
  }

  /// Loads [url], waits for its `ready`, makes [call] and returns the
  /// first [M] after it, within [limit], with how long it took.
  Future<(M, Duration)> _ask<M extends HostMessage>(
    Uri url,
    HostCall call,
    Duration limit,
    Future<void>? cancelled,
  ) async {
    final surface = _surfaceFor(url);
    final aid = _artifactOf(url);
    final ready = Completer<void>();
    final answer = Completer<M>();
    var asked = false;
    final sub = surface.messages.listen((m) {
      switch (m) {
        case Ready(:final artifact) when artifact == aid:
          if (!ready.isCompleted) ready.complete();
        case final M got when asked:
          if (!answer.isCompleted) answer.complete(got);
        case _:
          break;
      }
    });
    final faults = surface.faults.listen((f) {
      if (!f.reloading && !ready.isCompleted) {
        ready.completeError(InspectorError(f.description));
      }
    });
    Future<T> within<T>(Future<T> f, Duration limit, String why) =>
        Future.any<T>([
          f,
          _clock.delay(limit).then<T>((_) => throw InspectorError(why)),
          if (cancelled != null)
            cancelled.then<T>((_) => throw const InspectorError('cancelled')),
        ]);
    try {
      surface.setVisible(true);
      await surface.load(url);
      await within(
        ready.future,
        readyLimit,
        'it did not load within ${readyLimit.inSeconds} seconds',
      );
      asked = true;
      final started = _clock.now();
      await surface.call(call);
      final m = await within(
        answer.future,
        limit,
        'it did not answer within ${limit.inSeconds} seconds',
      );
      return (m, _clock.now().difference(started));
    } finally {
      unawaited(sub.cancel());
      unawaited(faults.cancel());
      // Leave nothing pending on the completers a fault or timeout skipped.
      ready.future.ignore();
      if (!_disposed) surface.setVisible(false);
    }
  }

  /// An artifact ID no artifact has, for [warm]'s load.
  static const _warmId = '000000000000';

  /// Makes the surface for the daemon at [base] and loads the shell's
  /// inspect view in it once, so the WebView, the shell and its chunks are
  /// warm before the first inspection; call it as the daemon comes up. It
  /// waits for any inspection in progress, and does nothing when the
  /// surface for [base] is already made.
  Future<void> warm(Uri base) {
    final prior = _last;
    final done = Completer<void>();
    _last = done.future;
    return () async {
      try {
        await prior;
        final b = Uri(scheme: base.scheme, host: base.host, port: base.port);
        if (_disposed || (_surface.value != null && b == _base)) return;
        final surface = _surfaceFor(b);
        surface.setVisible(false);
        await surface.load(
          b.replace(
            path: '/a/$_warmId',
            queryParameters: const {'app': '1', 'inspect': '1'},
          ),
        );
      } on Object {
        // The first inspection loads it anyway.
      } finally {
        done.complete();
      }
    }();
  }

  AppSurface _surfaceFor(Uri url) {
    final base = Uri(scheme: url.scheme, host: url.host, port: url.port);
    final current = _surface.value;
    if (current != null && base == _base) return current;
    current?.dispose();
    _base = base;
    return _surface.value = _make(base);
  }

  /// The artifact the inspect view at [url] shows (`/a/<id>…`).
  static String? _artifactOf(Uri url) {
    final segs = url.pathSegments;
    return segs.length >= 2 && segs[0] == 'a' ? segs[1] : null;
  }

  /// Ends the surface; later inspections fail.
  void dispose() {
    _disposed = true;
    _surface.value?.dispose();
    _surface.value = null;
    _surface.dispose();
  }
}

/// The size an inspection renders at: [screen] (the app's own size, which
/// an app screen fills), but on a desktop at most a phone's
/// [phone] (390 × 844 logical pixels), and [phone] when [screen] is empty.
Size inspectorSize(Size screen, {required bool desktop}) {
  const phone = Size(390, 844);
  if (screen.isEmpty) return phone;
  if (!desktop) return screen;
  return Size(
    math.min(screen.width, phone.width),
    math.min(screen.height, phone.height),
  );
}

/// Keeps [inspector]'s surface laid out at [size] at the top left of the
/// root `Stack`, under an opaque cover of [cover] and under [child] (the
/// app). It is never `Offstage`: a WebView that is not laid out does not
/// run its page.
class InspectorHost extends StatelessWidget {
  const InspectorHost({
    super.key,
    required this.inspector,
    required this.size,
    required this.cover,
    required this.child,
  });

  final WebViewInspector inspector;
  final Size size;
  final Color cover;
  final Widget child;

  @override
  Widget build(BuildContext context) => Stack(
    textDirection: TextDirection.ltr,
    children: [
      Positioned(
        left: 0,
        top: 0,
        width: size.width,
        height: size.height,
        child: ExcludeSemantics(
          child: IgnorePointer(
            child: ValueListenableBuilder<AppSurface?>(
              valueListenable: inspector.surface,
              builder: (context, s, _) => s == null
                  ? const SizedBox.shrink()
                  : KeyedSubtree(key: ObjectKey(s), child: s.build(context)),
            ),
          ),
        ),
      ),
      Positioned.fill(
        child: ColoredBox(
          key: const ValueKey('inspector-cover'),
          color: cover.withValues(alpha: 1),
        ),
      ),
      Positioned.fill(child: child),
    ],
  );
}
