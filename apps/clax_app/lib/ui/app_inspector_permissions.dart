/// What an app is allowed, as the app inspector's Permissions section
/// lists it: one [PermissionSource] per kind of permission, each listing
/// its [Permission]s for an app and revoking them.
///
/// The db read grants and the agent's remembered read consents are here.
/// Device capabilities, network hosts and AI use each add a source of
/// their own, of [PermissionSource.kind] `device`, `net` and `ai`; until
/// one is given, the section shows that kind as coming
/// ([plannedPermissionKinds]).
library;

import 'package:flutter/material.dart';

import '../clax/api.dart';
import 'read_consent_dialog.dart';

/// The app being inspected, and every app's title by ID, for naming the
/// other side of a permission.
class InspectedApp {
  const InspectedApp({
    required this.id,
    required this.title,
    required this.capabilities,
    required this.titles,
  });

  final String id;
  final String title;

  /// The app's declared capabilities (`ArtifactSummary.capabilities`).
  final Map<String, Object?> capabilities;

  /// Every app's title by ID.
  final Map<String, String> titles;

  /// The title of app [id], or a short form of the ID for an app that is
  /// gone.
  String titleOf(String id) => switch (titles[id]) {
    final t? when t.trim().isNotEmpty => t,
    _ => 'App ${id.length > 8 ? id.substring(0, 8) : id}',
  };
}

/// One thing an app may do or have, as one row of the Permissions section.
class Permission {
  const Permission({
    required this.label,
    this.detail,
    this.revoke,
    this.revokeLabel = 'Revoke',
  });

  /// What is allowed, in a short sentence.
  final String label;

  /// When or why it was allowed, or what revoking it means.
  final String? detail;

  /// Takes the permission back; null when it cannot be revoked here (a
  /// share another app declares, for one).
  final Future<void> Function()? revoke;
  final String revokeLabel;
}

/// One kind of permission: its heading in the Permissions section and the
/// app's permissions of that kind.
abstract interface class PermissionSource {
  /// A stable name for the kind: `db`, `agent`, `device`, `net`, `ai`.
  String get kind;

  /// The section heading, such as "Data from other apps".
  String get title;

  /// One line under the heading saying what the kind covers.
  String get description;

  IconData get icon;

  /// The app's permissions of this kind, newest last. An empty list shows
  /// "None".
  Future<List<Permission>> load(InspectedApp app);
}

/// The kinds the Permissions section expects, with what it shows for one
/// no [PermissionSource] covers yet.
const plannedPermissionKinds =
    <({String kind, String title, String description, IconData icon})>[
      (
        kind: 'device',
        title: 'Device',
        description: 'Notifications, sharing, files, camera and location.',
        icon: Icons.phone_android_rounded,
      ),
      (
        kind: 'net',
        title: 'Network',
        description: 'The web hosts this app may reach.',
        icon: Icons.public_rounded,
      ),
      (
        kind: 'ai',
        title: 'AI',
        description: 'Using the model from inside the app.',
        icon: Icons.auto_awesome_rounded,
      ),
    ];

/// The db read grants on both sides of the app: the other apps the owner
/// let it read, and the other apps the owner let read it, each revocable;
/// and the apps it shares its documents with by its own declaration
/// (`db.share.read`), which a revoke does not undo.
class DbGrantPermissions implements PermissionSource {
  const DbGrantPermissions(this.api);

  final ClaxApi api;

  @override
  String get kind => 'db';

  @override
  String get title => 'Data from other apps';

  @override
  String get description =>
      'Other apps’ data this app may read, and apps that may read this '
      'app’s data.';

  @override
  IconData get icon => Icons.swap_horiz_rounded;

  @override
  Future<List<Permission>> load(InspectedApp app) async {
    final mine = await api.readGrants(app.id);
    final others = await Future.wait([
      for (final other in app.titles.keys)
        if (other != app.id)
          api
              .readGrants(other)
              .then(
                (gs) =>
                    (other, gs.where((g) => g.sourceId == app.id).firstOrNull),
                onError: (Object _) => (other, null),
              ),
    ]);
    final shares = switch (app.capabilities['db']) {
      {'share': {'read': final List<Object?> ids}} => ids.whereType<String>(),
      _ => const <String>[],
    };
    return [
      for (final g in mine)
        Permission(
          label: 'Reads ${app.titleOf(g.sourceId)}',
          detail: 'You allowed this ${versionDay(g.grantedAt)}',
          revoke: () => api.revokeRead(app.id, g.sourceId),
        ),
      for (final (other, g) in others)
        if (g != null)
          Permission(
            label: '${app.titleOf(other)} reads this app',
            detail: 'You allowed this ${versionDay(g.grantedAt)}',
            revoke: () => api.revokeRead(other, app.id),
          ),
      for (final id in shares)
        Permission(
          label: '${app.titleOf(id)} reads this app',
          detail: 'This app shares its data with it',
        ),
    ];
  }
}

/// The agent's "Always for this app" read consents: while it works on this
/// app it may read another app's data, or while it works on another app it
/// may read this one's. Revoking one makes the agent ask again.
class AgentReadPermissions implements PermissionSource {
  const AgentReadPermissions(this.memory);

  final ConsentMemory memory;

  @override
  String get kind => 'agent';

  @override
  String get title => 'Agent reads';

  @override
  String get description =>
      'Data the agent may read while it builds, without asking again.';

  @override
  IconData get icon => Icons.smart_toy_outlined;

  @override
  Future<List<Permission>> load(InspectedApp app) async {
    final pairs = await memory.pairs();
    return [
      for (final (reader, source) in pairs)
        if (reader == app.id)
          Permission(
            label: 'Reads ${app.titleOf(source)} while building this app',
            revoke: () => memory.forget(reader, source),
          )
        else if (source == app.id)
          Permission(
            label: 'Reads this app while building ${app.titleOf(reader)}',
            revoke: () => memory.forget(reader, source),
          ),
    ];
  }
}

/// A day for a stamp: "on 12 Oct 2026", or the stamp itself when it does
/// not parse.
String versionDay(String stamp) {
  final t = DateTime.tryParse(stamp)?.toLocal();
  if (t == null) return stamp;
  const months = [
    'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
    'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
  ];
  return 'on ${t.day} ${months[t.month - 1]} ${t.year}';
}
