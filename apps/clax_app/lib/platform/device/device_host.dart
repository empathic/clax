/// The app's side of the device capabilities (`notify`, `share`, `file`,
/// `camera`, `location`; `web/contract/clax-extensions.d.ts`, `Device`):
/// each [DeviceRequest] the shell forwards is checked, asked of the person
/// once per app and capability, made through the ports, and answered with
/// a [DeviceResult].
library;

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import '../../ui/host_protocol.dart';
import 'device_grants.dart';
import 'device_ports.dart';
import 'reminders.dart';

export 'device_ports.dart' show DeviceError;
export 'reminders.dart' show ReminderTap;

/// The largest file a page may pick, share or save.
const deviceMaxBytes = 4 * 1024 * 1024;

/// Asks the person whether app [aid] may use [cap].
abstract interface class DeviceConsent {
  Future<bool> ask(String aid, String cap);
}

class DeviceHost {
  DeviceHost({
    required this.grants,
    required this.consent,
    required this.reminders,
    required this.share,
    required this.files,
    required this.camera,
    required this.location,
  });

  final DeviceGrants grants;
  final DeviceConsent consent;
  final Reminders reminders;
  final SharePort share;
  final FilesPort files;
  final CameraPort camera;
  final LocationPort location;

  NotifierPort get _notifier => reminders.port;

  final _taps = StreamController<ReminderTap>.broadcast();

  /// The consent asked now, by app and capability, so calls made together
  /// share one dialog.
  final _asking = <(String, String), Future<bool>>{};
  var _started = false;

  /// The reminders the person taps: open the app at the route.
  Stream<ReminderTap> get taps => _taps.stream;

  /// Sets up the notifications; the tap that launched the app, if any, is
  /// the first of [taps]. Once.
  Future<void> start() async {
    if (_started) return;
    _started = true;
    try {
      await _notifier.init((p) {
        if (ReminderTap.parse(p) case final t?) _taps.add(t);
      });
      if (ReminderTap.parse(await _notifier.launchPayload()) case final t?) {
        _taps.add(t);
      }
    } on Object {
      // No notifications here: notify answers unavailable.
    }
  }

  /// Forgets the person's answer for app [aid]'s [cap] (Allow or Don't
  /// allow), cancelling its reminders when it is `notify`.
  Future<void> revoke(String aid, String cap) async {
    await grants.revoke(aid, cap);
    if (cap == 'notify') await reminders.cancelAll(aid);
  }

  /// App [aid] was deleted: cancels its reminders and forgets its answers.
  Future<void> forget(String aid) async {
    await reminders.cancelAll(aid);
    await grants.forget(aid);
  }

  /// Makes request [r] of app [aid] and answers it.
  Future<DeviceResult> handle(String aid, DeviceRequest r) async {
    try {
      final value = await _call(aid, r);
      return DeviceResult.ok(r.id, value);
    } on DeviceError catch (e) {
      return DeviceResult.error(r.id, e.code, e.message);
    } on Object catch (e) {
      return DeviceResult.error(r.id, 'unavailable', '$e');
    }
  }

  Future<Object?> _call(String aid, DeviceRequest r) async {
    if (!deviceCaps.contains(r.cap)) {
      throw DeviceError(
        'capability_removed',
        '${r.cap} is not a device capability',
      );
    }
    final arg = r.args.firstOrNull;
    _checkMethod(r);
    await _allow(aid, r.cap);
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
        return {'shared': await share.text(arg)};
      case ('share', 'file'):
        final f = _file(arg);
        return {'shared': await share.file(f.name, f.mime, f.data)};
      case ('file', 'pick'):
        final picked = await files.pick(_extensions(arg));
        if (picked == null) {
          throw const DeviceError.cancelled('no file was picked');
        }
        if (picked.data.length > deviceMaxBytes) {
          throw const DeviceError('too_large', 'the file is over 4 MB');
        }
        return {
          'name': picked.name,
          'mime': picked.mime,
          'data': base64Encode(picked.data),
        };
      case ('file', 'save'):
        final f = _file(arg);
        return {'saved': await files.save(f.name, f.mime, f.data)};
      case ('camera', 'take' || 'pick'):
        final p = r.method == 'take'
            ? await camera.take()
            : await camera.pick();
        if (p == null) {
          throw const DeviceError.cancelled('no photo was taken or picked');
        }
        return {
          'mime': p.mime,
          'data': base64Encode(p.data),
          'width': p.width,
          'height': p.height,
        };
      case ('location', 'current'):
        final l = await location.current();
        return {'lat': l.lat, 'lon': l.lon, 'accuracy': l.accuracy};
    }
    throw DeviceError(
      'capability_removed',
      '${r.cap}.${r.method} is not part of this runtime',
    );
  }

  static const _methods = {
    'notify': ['schedule', 'cancel', 'list'],
    'share': ['text', 'file'],
    'file': ['pick', 'save'],
    'camera': ['take', 'pick'],
    'location': ['current'],
  };

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
  Future<void> _allow(String aid, String cap) async {
    final before = await grants.decision(aid, cap);
    if (before?.allowed == true) return;
    if (before != null) {
      throw DeviceError.notGranted(
        'the person did not allow $cap for this app',
      );
    }
    final key = (aid, cap);
    // The cleanup returns nothing: whenComplete would wait on a returned
    // future, and the removed value is this very one.
    final ok = await (_asking[key] ??= consent.ask(aid, cap).whenComplete(() {
      _asking.remove(key);
    }));
    if ((await grants.decision(aid, cap))?.allowed != ok) {
      await grants.decide(aid, cap, allowed: ok);
    }
    if (!ok) {
      throw DeviceError.notGranted(
        'the person did not allow $cap for this app',
      );
    }
  }

  static ({String name, String mime, Uint8List data}) _file(Object? f) {
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
      final bytes = base64Decode(data);
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
