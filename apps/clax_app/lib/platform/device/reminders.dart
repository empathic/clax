/// The `notify` capability's reminders: each app's reminders by the page's
/// own IDs, kept in a JSON file, each one or more native notifications (a
/// reminder repeating on some weekdays is one weekly notification per
/// weekday).
library;

import 'dart:convert';
import 'dart:io';

import 'device_ports.dart';

/// The first native notification ID the reminders use, clear of the app's
/// own notices (`BackgroundRunService.kt` uses 1 to 66535).
const firstReminderId = 1 << 20;

/// The most reminders one app keeps (a weekly one takes one alarm per
/// weekday; Android refuses an app's alarms past 500).
const maxReminders = 64;

/// What a tapped reminder asks: open app [aid] at [route] (`#…`, or the
/// start page when null).
class ReminderTap {
  const ReminderTap(this.aid, this.route);

  final String aid;
  final String? route;

  String toPayload() => jsonEncode({'aid': aid, 'route': route});

  /// The tap [payload] encodes; null when it is not a reminder's.
  static ReminderTap? parse(String? payload) {
    if (payload == null) return null;
    try {
      final j = jsonDecode(payload);
      if (j is! Map || j['aid'] is! String) return null;
      final route = j['route'];
      return ReminderTap(j['aid'] as String, route is String ? route : null);
    } on FormatException {
      return null;
    }
  }

  @override
  bool operator ==(Object other) =>
      other is ReminderTap && other.aid == aid && other.route == route;

  @override
  int get hashCode => Object.hash(aid, route);
}

/// The next time at [hour]:[minute] local time after [now], on [weekday]
/// (1 Monday … 7 Sunday) when given.
DateTime nextAt(DateTime now, int hour, int minute, {int? weekday}) {
  var days = weekday == null ? 0 : (weekday - now.weekday) % 7;
  var t = DateTime(now.year, now.month, now.day + days, hour, minute);
  if (!t.isAfter(now)) {
    days += weekday == null ? 1 : 7;
    t = DateTime(now.year, now.month, now.day + days, hour, minute);
  }
  return t;
}

/// The most notifications [Reminders] keeps pending on iOS, which keeps
/// only the 64 nearest an app schedules and drops the rest silently; the
/// four left over are the app's own notices' room.
const iosPendingCap = 60;

class Reminders {
  Reminders(
    this.port, {
    this.file,
    DateTime Function()? now,
    int? pendingCap,
  }) : _now = now ?? DateTime.now,
       pendingCap = pendingCap ?? (Platform.isIOS ? iosPendingCap : null);

  final NotifierPort port;

  /// The most native notifications pending at once ([iosPendingCap] on
  /// iOS); none when null. With a cap, the nearest next times are pending
  /// and [topUp] schedules the rest as those fire.
  final int? pendingCap;

  /// Where the reminders are kept (`<support>/device-reminders.json`); in
  /// memory when null.
  final File? file;
  final DateTime Function() _now;

  Map<String, Object?>? _memory;

  Map<String, Object?> _load() {
    if (file == null) return _memory ??= {'next': firstReminderId, 'items': []};
    try {
      final j = jsonDecode(file!.readAsStringSync());
      if (j is Map && j['next'] is int && j['items'] is List) {
        return j.cast<String, Object?>();
      }
    } on FileSystemException {
      // None kept yet.
    } on FormatException {
      // Broken: start again.
    }
    return {'next': firstReminderId, 'items': <Object?>[]};
  }

  void _save(Map<String, Object?> book) {
    final f = file;
    if (f == null) {
      _memory = book;
      return;
    }
    f.parent.createSync(recursive: true);
    final tmp = File('${f.path}.tmp');
    tmp.writeAsStringSync(jsonEncode(book), flush: true);
    tmp.renameSync(f.path);
  }

  static List<Map<String, Object?>> _items(Map<String, Object?> book) => [
    for (final i in book['items'] as List)
      if (i is Map) i.cast<String, Object?>(),
  ];

  /// Schedules reminder [r] (`Device.Reminder`) for app [aid], replacing
  /// one of the same ID. Throws [DeviceError] `bad_request` for a reminder
  /// of the wrong shape or one in the past.
  Future<Map<String, Object?>> schedule(String aid, Object? r) async {
    final spec = _check(r);
    final book = _load();
    final items = _items(book);
    if (items.where((i) => i['aid'] == aid && i['id'] != spec['id']).length >=
        maxReminders) {
      throw const DeviceError.badRequest(
        'an app keeps at most $maxReminders reminders: cancel one first',
      );
    }
    for (final old in items.where(
      (i) => i['aid'] == aid && i['id'] == spec['id'],
    )) {
      for (final n in old['native'] as List) {
        await port.cancel(n as int);
      }
    }
    items.removeWhere((i) => i['aid'] == aid && i['id'] == spec['id']);
    var next = book['next'] as int;
    final natives = <int>[];
    final now = _now();
    final kept = {...spec, 'aid': aid, 'native': natives};
    final count = spec['at'] is String
        ? 1
        : ((spec['every'] as Map)['weekdays'] as List?)?.length ?? 1;
    for (var i = 0; i < count; i++) {
      natives.add(next++);
    }
    if (pendingCap == null) {
      for (final n in _natives(kept, now)) {
        await _post(n);
      }
    }
    items.add(kept);
    _save({'next': next, 'items': items});
    await topUp();
    return {'id': spec['id']};
  }

  /// With [pendingCap], makes the port's pending reminders the nearest
  /// [pendingCap] next times of every app's: cancels pending ones past the
  /// cap and schedules those now within it. Called at launch, after each
  /// schedule and cancel, and when a reminder is tapped. Without a cap it
  /// does nothing.
  Future<void> topUp() async {
    final cap = pendingCap;
    if (cap == null) return;
    final now = _now();
    final all = [
      for (final i in _items(_load())) ..._natives(i, now),
    ]..sort((a, b) => a.at.compareTo(b.at));
    final wanted = all.take(cap).toList();
    final wantedIds = {for (final n in wanted) n.id};
    final pending = await port.pendingIds();
    for (final n in all) {
      if (!wantedIds.contains(n.id) && pending.contains(n.id)) {
        await port.cancel(n.id);
      }
    }
    for (final n in wanted) {
      if (!pending.contains(n.id)) await _post(n);
    }
  }

  /// Item [i]'s native notifications still to come after [now], each at
  /// its next time.
  static List<_Native> _natives(Map<String, Object?> i, DateTime now) {
    final ids = (i['native'] as List).cast<int>();
    final payload = ReminderTap(
      i['aid'] as String,
      i['route'] as String?,
    ).toPayload();
    final title = i['title'] as String;
    final body = i['body'] as String? ?? '';
    if (i['at'] case final String at) {
      final t = DateTime.parse(at).toLocal();
      if (!t.isAfter(now)) return const [];
      return [_Native(ids.single, title, body, t, null, payload)];
    }
    final every = (i['every'] as Map).cast<String, Object?>();
    final hour = every['hour'] as int;
    final minute = every['minute'] as int;
    final weekdays = (every['weekdays'] as List?)?.cast<int>();
    final days = weekdays ?? const <int?>[null];
    return [
      for (var k = 0; k < days.length; k++)
        _Native(
          ids[k],
          title,
          body,
          nextAt(now, hour, minute, weekday: days[k]),
          days[k] == null ? Repeat.daily : Repeat.weekly,
          payload,
        ),
    ];
  }

  Future<void> _post(_Native n) => port.schedule(
    id: n.id,
    title: n.title,
    body: n.body,
    at: n.at,
    repeat: n.repeat,
    payload: n.payload,
  );

  /// Cancels app [aid]'s reminder [id].
  Future<Map<String, Object?>> cancel(String aid, Object? id) async {
    if (id is! String) {
      throw const DeviceError.badRequest('cancel takes a reminder ID');
    }
    final book = _load();
    final items = _items(book);
    final gone = items.where((i) => i['aid'] == aid && i['id'] == id).toList();
    for (final g in gone) {
      for (final n in g['native'] as List) {
        await port.cancel(n as int);
      }
    }
    items.removeWhere(gone.contains);
    if (gone.isNotEmpty) {
      _save({...book, 'items': items});
      await topUp();
    }
    return {'cancelled': gone.isNotEmpty};
  }

  /// App [aid]'s reminders still to come, as the page gave them.
  Future<List<Map<String, Object?>>> list(String aid) async {
    final now = _now();
    return [
      for (final i in _items(_load()))
        if (i['aid'] == aid &&
            (i['at'] is! String ||
                DateTime.parse(i['at'] as String).isAfter(now)))
          {
            for (final k in const [
              'id',
              'title',
              'body',
              'at',
              'every',
              'route',
            ])
              if (i[k] != null) k: i[k],
          },
    ];
  }

  /// Cancels every reminder of app [aid] (its `notify` grant was revoked).
  Future<void> cancelAll(String aid) async {
    for (final r in await list(aid)) {
      await cancel(aid, r['id']);
    }
  }

  /// The reminder [r] as kept: its fields checked, `at` as a UTC ISO time.
  Map<String, Object?> _check(Object? r) {
    Never bad(String why) => throw DeviceError.badRequest(why);
    if (r is! Map) bad('schedule takes {id, title, body?, at | every, route?}');
    final id = r['id'];
    if (id is! String || id.isEmpty || id.length > 64) {
      bad('id is a string of 1 to 64 characters');
    }
    final title = r['title'];
    if (title is! String || title.trim().isEmpty || title.length > 200) {
      bad('title is a string of 1 to 200 characters');
    }
    final body = r['body'];
    if (body != null && (body is! String || body.length > 1000)) {
      bad('body is a string of at most 1000 characters');
    }
    final route = r['route'];
    if (route != null &&
        (route is! String || !route.startsWith('#') || route.length > 512)) {
      bad('route is a fragment: "#" and at most 511 more characters');
    }
    final at = r['at'];
    final every = r['every'];
    if ((at == null) == (every == null)) bad('give one of at and every');
    final out = <String, Object?>{
      'id': id,
      'title': title,
      'body': ?body,
      'route': ?route,
    };
    if (at != null) {
      final t = at is String ? DateTime.tryParse(at) : null;
      if (t == null) bad('at is an ISO 8601 date-time');
      if (!t.isAfter(_now())) bad('at is in the past');
      out['at'] = t.toUtc().toIso8601String();
      return out;
    }
    if (every is! Map) bad('every is {hour, minute, weekdays?}');
    final hour = _int(every['hour']);
    final minute = _int(every['minute']);
    if (hour == null || hour < 0 || hour > 23) bad('every.hour is 0 to 23');
    if (minute == null || minute < 0 || minute > 59) {
      bad('every.minute is 0 to 59');
    }
    final days = every['weekdays'];
    List<int>? weekdays;
    if (days != null) {
      final ds = days is List ? days.map(_int).toList() : null;
      if (ds == null ||
          ds.isEmpty ||
          ds.any((d) => d == null || d < 1 || d > 7)) {
        bad('every.weekdays is a list of 1 (Monday) to 7 (Sunday)');
      }
      weekdays = ({for (final d in ds) d!}.toList())..sort();
    }
    out['every'] = {'hour': hour, 'minute': minute, 'weekdays': ?weekdays};
    return out;
  }

  static int? _int(Object? v) => switch (v) {
    final int n => n,
    final double d when d == d.roundToDouble() => d.toInt(),
    _ => null,
  };
}

/// One native notification of a reminder, at its next time.
class _Native {
  const _Native(this.id, this.title, this.body, this.at, this.repeat, this.payload);
  final int id;
  final String title;
  final String body;
  final DateTime at;
  final Repeat? repeat;
  final String payload;
}
