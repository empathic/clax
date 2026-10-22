/// The app's side of the device capabilities (`notify`, `share`, `file`,
/// `camera`, `location`; `web/contract/clax-extensions.d.ts`, `Device`):
/// each [DeviceRequest] the shell forwards is checked, asked of the person
/// once per app and capability, made through the ports, and answered with
/// a [DeviceResult].
///
/// Location is asked as Precise, Approximate or Don't allow. An app allowed
/// Approximate (or any app, when Android lets Clax have only an approximate
/// location) gets fixes snapped to a cell of about 2 km ([Coarsener]) on a
/// grid shifted at random once per install ([DeviceGrants.gridShift]),
/// marked `precise: false`. `location.watch` follows the device for the
/// app screen's [DeviceView] while the screen shows the app: the screen
/// stops it ([DeviceHost.stopWatches]) when it leaves, and it does not
/// start again by itself.
///
/// `grant` (any capability but `notify`) asks the person as a call would
/// and answers only the grant, `{allowed}`, with `precise` and the grid's
/// `shift` for location: on iOS the shell makes camera, files, share and
/// location calls through WebKit itself (`web/shell/src/caps/webkit.ts`),
/// so a host there has no [share], [files], [camera] or [location] port,
/// and a call that would need one answers `unavailable`.
library;

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/foundation.dart';

import '../clock.dart';
import '../offload.dart';
import '../../ui/host_protocol.dart';
import 'device_grants.dart';
import 'device_ports.dart';
import 'location_coarsen.dart';
import 'reminders.dart';

export 'device_ports.dart'
    show DeviceError, LocationFix, LocationSettingsPage, Precision;
export 'reminders.dart' show ReminderTap;

/// The largest file a page may pick, share or save.
const deviceMaxBytes = 4 * 1024 * 1024;

/// The person's answer to a device consent: Don't allow, Allow (for
/// location: Precise), for location only Approximate, or no answer (the
/// question dismissed), which is not kept: the next call asks again.
enum DeviceAnswer { deny, allow, approximate, dismissed }

/// Asks the person whether app [aid] may use [cap].
abstract interface class DeviceConsent {
  Future<DeviceAnswer> ask(String aid, String cap);
}

/// One app screen's side of the device calls that outlive their answer
/// (`location.watch`): [send] carries each watch's updates and its end to
/// the page, and [shown] says whether the screen shows the app once its
/// layout has settled (a watch that finishes starting while it does not is
/// stopped at once).
class DeviceView {
  DeviceView({required this.send, required this.shown});

  final void Function(DeviceEvent e) send;
  final Future<bool> Function() shown;

  /// How many watches run now; the screen shows that location is in use
  /// while any does.
  final watching = ValueNotifier<int>(0);

  final _watches = <String, _Watch>{};
}

enum _Phase { asking, starting, running }

class _Watch {
  _Watch(this.id, this.aid);
  final String id;
  final String aid;
  var phase = _Phase.asking;
  var cancelled = false;
  var precision = Precision.precise;
  late Coarsener grid;
  StreamSubscription<LocationFix>? sub;

  /// When the last update went to the page.
  DateTime? lastAt;

  /// Precise: the latest fix held back by the throttle, sent at the end of
  /// its window.
  LocationFix? trailing;

  /// Approximate: the cell last reported, and the one the device has moved
  /// well into since, when it did, and that cell's position.
  (int, int)? cell;
  (int, int)? pending;
  DateTime? pendingSince;
  LocationFix? pendingFix;

  /// Bumped to drop a scheduled send.
  var timer = 0;
}

class DeviceHost {
  DeviceHost({
    required this.grants,
    required this.consent,
    required this.reminders,
    this.share,
    this.files,
    this.camera,
    this.location,
    math.Random? random,
    Clock? clock,
  }) : _random = random ?? math.Random.secure(),
       clock = clock ?? const SystemClock();

  final DeviceGrants grants;
  final DeviceConsent consent;
  final Reminders reminders;

  /// The native ports; null where WebKit serves the capability (iOS).
  final SharePort? share;
  final FilesPort? files;
  final CameraPort? camera;
  final LocationPort? location;

  static T _port<T>(T? p, String cap) =>
      p ??
      (throw DeviceError.unavailable(
        '$cap runs through the web view here; the app has no native $cap',
      ));

  /// Draws the install's grid shift, the first time it is needed.
  final math.Random _random;
  final Clock clock;
  DateTime _now() => clock.now();

  /// An approximate watch reports a new cell only once the device has
  /// been well inside it for [approxHold], and at most once per
  /// [approxInterval] (Android's own approximate cadence), so neither the
  /// time nor the place of an update marks where an edge was crossed.
  static const approxHold = Duration(minutes: 3);
  static const approxInterval = Duration(minutes: 10);

  /// The app screens with watches, by app, so a revoke stops them.
  final _views = <String, Set<DeviceView>>{};

  /// The watches a view may run at once.
  static const maxWatches = 4;

  /// The least time between two updates of a watch.
  static const minInterval = Duration(seconds: 1);

  /// A watch's distance filter when the page gives none, and its bounds,
  /// in metres.
  static const defaultDistance = 10.0;
  static const minDistance = 5.0;
  static const maxDistance = 10000.0;

  NotifierPort get _notifier => reminders.port;

  final _taps = StreamController<ReminderTap>.broadcast();

  /// The consent asked now, by app and capability, so calls made together
  /// share one dialog.
  final _asking = <(String, String), Future<DeviceAnswer>>{};
  var _started = false;

  /// The reminders the person taps: open the app at the route.
  Stream<ReminderTap> get taps => _taps.stream;

  /// Sets up the notifications; the tap that launched the app, if any, is
  /// the first of [taps]. Once. Tops the pending reminders up
  /// ([Reminders.topUp]) now and after each tap, since a reminder shown is
  /// no longer pending.
  Future<void> start() async {
    if (_started) return;
    _started = true;
    try {
      await _notifier.init((p) {
        if (ReminderTap.parse(p) case final t?) {
          _taps.add(t);
          unawaited(reminders.topUp().catchError((Object _) {}));
        }
      });
      if (ReminderTap.parse(await _notifier.launchPayload()) case final t?) {
        _taps.add(t);
      }
      await reminders.topUp();
    } on Object {
      // No notifications here: notify answers unavailable.
    }
  }

  /// Forgets the person's answer for app [aid]'s [cap] (Allow or Don't
  /// allow), cancelling its reminders when it is `notify`.
  Future<void> revoke(String aid, String cap) async {
    await grants.revoke(aid, cap);
    if (cap == 'location') _stopApp(aid);
    if (cap == 'notify') await reminders.cancelAll(aid);
  }

  /// App [aid] was deleted: cancels its reminders and forgets its answers.
  Future<void> forget(String aid) async {
    _stopApp(aid);
    await reminders.cancelAll(aid);
    await grants.forget(aid);
  }

  /// Makes request [r] of app [aid] and answers it; [view] is the app
  /// screen's, which `location.watch` needs.
  Future<DeviceResult> handle(
    String aid,
    DeviceRequest r, {
    DeviceView? view,
  }) async {
    try {
      final value = await _call(aid, r, view);
      return DeviceResult.ok(r.id, value);
    } on DeviceError catch (e) {
      return DeviceResult.error(r.id, e.code, e.message);
    } on Object catch (e) {
      return DeviceResult.error(r.id, 'unavailable', '$e');
    }
  }

  Future<Object?> _call(String aid, DeviceRequest r, DeviceView? view) async {
    if (!deviceCaps.contains(r.cap)) {
      throw DeviceError(
        'capability_removed',
        '${r.cap} is not a device capability',
      );
    }
    final arg = r.args.firstOrNull;
    _checkMethod(r);
    switch ((r.cap, r.method)) {
      case ('location', 'watch'):
        return _watch(aid, arg, view);
      case (_, 'grant'):
        return _grant(aid, r.cap);
      case ('location', 'unwatch'):
        return {'stopped': view != null && _unwatch(view, arg)};
    }
    final decision = await _allow(aid, r.cap);
    switch ((r.cap, r.method)) {
      case ('notify', 'schedule'):
        if (!await _notifier.requestPermission()) {
          throw const DeviceError.notGranted(
            'notifications are off for Clax in system settings',
          );
        }
        return reminders.schedule(aid, arg);
      case ('notify', 'cancel'):
        return reminders.cancel(aid, arg);
      case ('notify', 'list'):
        return reminders.list(aid);
      case ('share', 'text'):
        if (arg is! String || arg.isEmpty) {
          throw const DeviceError.badRequest('text takes a non-empty string');
        }
        return {'shared': await _port(share, 'share').text(arg)};
      case ('share', 'file'):
        final f = await _file(arg);
        return {
          'shared': await _port(share, 'share').file(f.name, f.mime, f.data),
        };
      case ('file', 'pick'):
        final picked = await _port(files, 'file').pick(_extensions(arg));
        if (picked == null) {
          throw const DeviceError.cancelled('no file was picked');
        }
        if (picked.data.length > deviceMaxBytes) {
          throw const DeviceError('too_large', 'the file is over 4 MB');
        }
        return {
          'name': picked.name,
          'mime': picked.mime,
          'data': await base64Off(picked.data),
        };
      case ('file', 'save'):
        final f = await _file(arg);
        return {
          'saved': await _port(files, 'file').save(f.name, f.mime, f.data),
        };
      case ('camera', 'take' || 'pick'):
        final c = _port(camera, 'camera');
        final p = r.method == 'take' ? await c.take() : await c.pick();
        if (p == null) {
          throw const DeviceError.cancelled('no photo was taken or picked');
        }
        return {
          'mime': p.mime,
          'data': await base64Off(p.data),
          'width': p.width,
          'height': p.height,
        };
      case ('location', 'current'):
        final p = decision.precision ?? Precision.precise;
        final grid = await _grid();
        final fix = await _port(location, 'location').current(p);
        return _fixJson(_shown(fix, p, grid));
    }
    throw DeviceError(
      'capability_removed',
      '${r.cap}.${r.method} is not part of this runtime',
    );
  }

  static const _methods = {
    'notify': ['schedule', 'cancel', 'list'],
    'share': ['text', 'file', 'grant'],
    'file': ['pick', 'save', 'grant'],
    'camera': ['take', 'pick', 'grant'],
    'location': ['current', 'watch', 'unwatch', 'grant'],
  };

  /// `cap.grant()`: the person's answer for app [aid], asked as a call of
  /// [cap] would ask it, and nothing made. `{allowed: false}` for Don't
  /// allow; for location `precise` and the install's grid `shift`
  /// (`[lat, lon]`), which the shell snaps an approximate fix on.
  Future<Object?> _grant(String aid, String cap) async {
    final DeviceDecision d;
    try {
      d = await _allow(aid, cap);
    } on DeviceError catch (e) {
      if (e.code == 'not_granted') return {'allowed': false};
      rethrow;
    }
    if (cap != 'location') return {'allowed': true};
    final grid = await _grid();
    return {
      'allowed': true,
      'precise': (d.precision ?? Precision.precise) == Precision.precise,
      'shift': [grid.offsetLat, grid.offsetLon],
    };
  }

  static void _checkMethod(DeviceRequest r) {
    if (!(_methods[r.cap]?.contains(r.method) ?? false)) {
      throw DeviceError(
        'capability_removed',
        '${r.cap}.${r.method} is not part of this runtime',
      );
    }
  }

  /// Asks the person unless they answered for app [aid] and [cap] before;
  /// throws `not_granted` when they do not allow it. The answer is kept
  /// either way, until revoked.
  Future<DeviceDecision> _allow(String aid, String cap) async {
    final before = await grants.decision(aid, cap);
    if (before?.allowed == true) return before!;
    if (before != null) {
      throw DeviceError.notGranted(
        'the person did not allow $cap for this app',
      );
    }
    final key = (aid, cap);
    // The cleanup returns nothing: whenComplete would wait on a returned
    // future, and the removed value is this very one.
    final answer = await (_asking[key] ??= consent.ask(aid, cap).whenComplete(
      () {
        _asking.remove(key);
      },
    ));
    if (answer == DeviceAnswer.dismissed) {
      throw const DeviceError(
        'dismissed',
        'the person closed the question without answering; ask again from '
            'a tap',
      );
    }
    final ok = answer != DeviceAnswer.deny;
    final precision = answer == DeviceAnswer.approximate
        ? Precision.approximate
        : Precision.precise;
    final kept = await grants.decision(aid, cap);
    if (kept == null || kept.allowed != ok) {
      await grants.decide(aid, cap, allowed: ok, precision: precision);
    }
    if (!ok) {
      throw DeviceError.notGranted(
        'the person did not allow $cap for this app',
      );
    }
    return (await grants.decision(aid, cap)) ??
        DeviceDecision(
          allowed: true,
          at: _now(),
          precision: cap == 'location' ? precision : null,
        );
  }

  // --- location ---

  Future<Coarsener>? _gridOnce;

  /// The install's grid.
  Future<Coarsener> _grid() => _gridOnce ??= grants
      .gridShift(() => (_random.nextDouble(), _random.nextDouble()))
      .then((s) => Coarsener(offsetLat: s.$1, offsetLon: s.$2));

  /// [fix] as an app allowed [precision] sees it: snapped to its cell of
  /// [grid] unless both the app and the system allow precise.
  LocationFix _shown(LocationFix fix, Precision precision, Coarsener grid) =>
      fix.precise && precision == Precision.precise ? fix : grid.coarsen(fix);

  static Map<String, Object?> _fixJson(LocationFix f) => {
    'lat': f.lat,
    'lon': f.lon,
    'accuracy': f.accuracy,
    'precise': f.precise,
  };

  static const _stoppedMessage =
      'location stopped because the app left the screen; start it again '
      'from a tap';

  /// Starts `location.watch` `{watch, minDistance?}` for [view]: answers
  /// `{watching: true}` once updates flow, which [view] then gets as
  /// [DeviceEvent]s under the page's `watch` ID.
  Future<Object?> _watch(String aid, Object? arg, DeviceView? view) async {
    if (view == null) {
      throw const DeviceError.unavailable(
        'location.watch runs only in an app screen',
      );
    }
    final id = arg is Map ? arg['watch'] : null;
    if (id is! String || id.isEmpty || id.length > 64) {
      throw const DeviceError.badRequest(
        'watch takes {watch, minDistance?} with watch an ID of 1 to 64 '
        'characters',
      );
    }
    final md = (arg as Map)['minDistance'];
    if (md != null && (md is! num || !md.isFinite)) {
      throw const DeviceError.badRequest('minDistance is a number of metres');
    }
    final distance = ((md as num?)?.toDouble() ?? defaultDistance).clamp(
      minDistance,
      maxDistance,
    );
    if (view._watches.containsKey(id)) {
      throw DeviceError.badRequest('watch $id is already running');
    }
    if (view._watches.length >= maxWatches) {
      throw const DeviceError.badRequest(
        'at most $maxWatches location watches at once',
      );
    }
    final port = _port(location, 'location');
    final w = _Watch(id, aid);
    view._watches[id] = w;
    (_views[aid] ??= {}).add(view);
    try {
      final d = await _allow(aid, 'location');
      if (w.cancelled) throw const DeviceError('stopped', _stoppedMessage);
      w
        ..phase = _Phase.starting
        ..precision = d.precision ?? Precision.precise
        ..grid = await _grid();
      final stream = await port.watch(w.precision, minDistance: distance);
      if (w.cancelled || !await view.shown() || w.cancelled) {
        throw const DeviceError('stopped', _stoppedMessage);
      }
      w
        ..phase = _Phase.running
        ..sub = stream.listen(
          (f) => _update(view, w, f),
          onError: (Object e) => _end(
            view,
            w,
            e is DeviceError ? e : DeviceError.unavailable('$e'),
          ),
        );
      _count(view);
      return {'watching': true};
    } on Object {
      if (view._watches[id] == w) view._watches.remove(id);
      rethrow;
    }
  }

  /// A fix for watch [w]. Precise: sent, or when one went less than
  /// [minInterval] ago, held and sent (the latest held) as the window
  /// ends. Approximate: the first sent at once; after it a new cell only
  /// once the device has been [Coarsener.margin] inside it for
  /// [approxHold], and at most once per [approxInterval].
  void _update(DeviceView view, _Watch w, LocationFix fix) {
    if (w.cancelled) return;
    final shown = _shown(fix, w.precision, w.grid);
    if (shown.precise) {
      _precise(view, w, shown);
    } else {
      _approximate(view, w, fix, shown);
    }
  }

  void _send(DeviceView view, _Watch w, LocationFix f) {
    w.lastAt = _now();
    view.send(DeviceEvent.update(w.id, _fixJson(f)));
  }

  /// Runs [f] after [d] on [clock], unless [w] ends or schedules again.
  void _after(_Watch w, Duration d, void Function() f) {
    final t = ++w.timer;
    unawaited(
      clock.delay(d).then((_) {
        if (!w.cancelled && t == w.timer) f();
      }),
    );
  }

  void _precise(DeviceView view, _Watch w, LocationFix f) {
    final last = w.lastAt;
    final now = _now();
    if (last == null || now.difference(last) >= minInterval) {
      w
        ..trailing = null
        ..timer += 1;
      _send(view, w, f);
      return;
    }
    final waiting = w.trailing != null;
    w.trailing = f;
    if (waiting) return;
    _after(w, last.add(minInterval).difference(now), () {
      if (w.trailing case final t?) {
        w.trailing = null;
        _send(view, w, t);
      }
    });
  }

  void _approximate(
    DeviceView view,
    _Watch w,
    LocationFix fix,
    LocationFix shown,
  ) {
    final c = w.grid.cell(fix.lat, fix.lon);
    final key = (c.row, c.col);
    if (w.cell == null) {
      w.cell = key;
      _send(view, w, shown);
      return;
    }
    if (key == w.cell) {
      // Back in the reported cell: whatever was pending is off.
      w
        ..pending = null
        ..timer += 1;
      return;
    }
    // Near an edge of another cell, or still in the pending one: no change.
    if (!c.deep || key == w.pending) return;
    w
      ..pending = key
      ..pendingSince = _now()
      ..pendingFix = shown;
    _approxDue(view, w);
  }

  /// Reports the pending cell once it is due, or waits until it is.
  void _approxDue(DeviceView view, _Watch w) {
    final since = w.pendingSince, fix = w.pendingFix;
    if (w.pending == null || since == null || fix == null) return;
    var due = since.add(approxHold);
    if (w.lastAt case final last?) {
      final next = last.add(approxInterval);
      if (next.isAfter(due)) due = next;
    }
    final now = _now();
    if (!now.isBefore(due)) {
      w
        ..cell = w.pending
        ..pending = null;
      _send(view, w, fix);
      return;
    }
    _after(w, due.difference(now), () => _approxDue(view, w));
  }

  /// Watch [w] failed with [e]: it ends, and the page hears why.
  void _end(DeviceView view, _Watch w, DeviceError e) {
    if (w.cancelled) return;
    _drop(view, w);
    view.send(DeviceEvent.end(w.id, e.code, e.message));
  }

  void _drop(DeviceView view, _Watch w) {
    w.cancelled = true;
    unawaited(w.sub?.cancel());
    if (view._watches[w.id] == w) view._watches.remove(w.id);
    if (!view._watches.values.any((o) => o.aid == w.aid)) {
      _views[w.aid]?.remove(view);
    }
    _count(view);
  }

  /// App [aid]'s location was revoked, or the app deleted: its watches end
  /// with `not_granted`, those still asking or starting included.
  void _stopApp(String aid) {
    for (final view in (_views.remove(aid) ?? const <DeviceView>{}).toList()) {
      for (final w in view._watches.values.toList()) {
        if (w.aid != aid) continue;
        final running = w.phase == _Phase.running;
        _drop(view, w);
        if (running) {
          view.send(
            DeviceEvent.end(
              w.id,
              'not_granted',
              'location is no longer allowed for this app',
            ),
          );
        }
      }
    }
  }

  void _count(DeviceView view) => view.watching.value = view._watches.values
      .where((w) => w.phase == _Phase.running)
      .length;

  /// The page stopped watch `{watch}`: no end is sent. Whether it ran.
  bool _unwatch(DeviceView view, Object? arg) {
    final id = arg is Map ? arg['watch'] : null;
    final w = id is String ? view._watches[id] : null;
    if (w == null) return false;
    _drop(view, w);
    return true;
  }

  /// Stops [view]'s watches, each ending in the page with `stopped`
  /// ([message], when given): those running or starting, and with
  /// [asking] those still waiting on the person's consent too (the page
  /// that made them is gone).
  void stopWatches(DeviceView view, {bool asking = false, String? message}) {
    for (final w in view._watches.values.toList()) {
      if (w.phase == _Phase.asking && !asking) continue;
      final running = w.phase == _Phase.running;
      _drop(view, w);
      if (running) {
        view.send(DeviceEvent.end(w.id, 'stopped', message ?? _stoppedMessage));
      }
    }
  }

  /// Opens the system settings that clear a location error [code]: the
  /// device's location switch for `location_off`, Clax's own permissions
  /// otherwise.
  Future<void> openLocationSettings(String code) async =>
      location?.openSettings(
    code == 'location_off'
        ? LocationSettingsPage.location
        : LocationSettingsPage.app,
  );

  static Future<({String name, String mime, Uint8List data})> _file(
    Object? f,
  ) async {
    if (f is! Map || f['name'] is! String || f['data'] is! String) {
      throw const DeviceError.badRequest(
        'takes {name, mime, data} with data as base64',
      );
    }
    final name = (f['name'] as String).trim();
    if (name.isEmpty ||
        name.length > 255 ||
        name.contains('/') ||
        name.contains(r'\')) {
      throw const DeviceError.badRequest(
        'name is a file name of 1 to 255 characters',
      );
    }
    final data = f['data'] as String;
    if (data.length > deviceMaxBytes * 4 ~/ 3 + 4) {
      throw const DeviceError('too_large', 'the file is over 4 MB');
    }
    try {
      final bytes = await base64DecodeOff(data);
      final mime = f['mime'];
      return (
        name: name,
        mime: mime is String && mime.contains('/')
            ? mime
            : 'application/octet-stream',
        data: bytes,
      );
    } on FormatException {
      throw const DeviceError.badRequest('data is not base64');
    }
  }

  /// The extensions of `file.pick`'s `{accept}` (`csv`, `.csv`), and
  /// those of the MIME types it names; empty for any file.
  static List<String> _extensions(Object? opts) {
    final accept = opts is Map ? opts['accept'] : null;
    if (accept == null) return const [];
    if (accept is! List || accept.any((a) => a is! String)) {
      throw const DeviceError.badRequest(
        'accept is a list of extensions or MIME types',
      );
    }
    final out = <String>{};
    for (final a in accept.cast<String>()) {
      final s = a.trim().toLowerCase();
      if (s.contains('/')) {
        // A MIME type: its known extensions; a wildcard allows any file.
        final known = _mimeExtensions[s];
        if (known == null) return const [];
        out.addAll(known);
      } else if (s.isNotEmpty) {
        out.add(s.startsWith('.') ? s.substring(1) : s);
      }
    }
    return out.toList();
  }

  static const _mimeExtensions = {
    'text/csv': ['csv'],
    'text/plain': ['txt'],
    'text/markdown': ['md'],
    'application/json': ['json'],
    'application/pdf': ['pdf'],
    'image/png': ['png'],
    'image/jpeg': ['jpg', 'jpeg'],
    'image/gif': ['gif'],
    'image/webp': ['webp'],
  };
}
