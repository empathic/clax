/// What the app screen offers for a linked app (spec
/// 2026-10-21-app-push-pull §5.3, §5.7, §5.11).
library;

import 'package:flutter/foundation.dart' show ValueListenable;
import 'package:flutter/widgets.dart' show BuildContext;

import '../../bundle/lineage_store.dart';
import '../../remote/checker.dart';

/// A linked app's hooks for its screen: the status the checker keeps, its
/// lineage (the link, for the Versions marks), and the publish, update and
/// held-reply actions. An action left null is not offered.
class LinkedAppHooks {
  const LinkedAppHooks({
    required this.status,
    required this.lineage,
    this.opened,
    this.publish,
    this.update,
    this.diverged,
    this.heldReply,
    this.heldNote,
    this.publishAndReply,
  });

  /// Each linked app's status ([LinkedChecker.status]).
  final ValueListenable<Map<String, LinkedStatus>> status;

  /// App `aid`'s lineage entry, or null.
  final Future<LineageEntry?> Function(String aid) lineage;

  /// App `aid`'s screen opened ([LinkedChecker.onOpen]).
  final Future<void> Function(String aid)? opened;

  /// Publishes local version `version` of app `aid` (the publish flow).
  final Future<void> Function(BuildContext context, String aid, int version)?
  publish;

  /// Offers app `aid`'s waiting update (the update sheet, then the
  /// update).
  final Future<void> Function(BuildContext context, String aid)? update;

  /// Offers app `aid`'s diverged choices (the diverged sheet, then the
  /// choice).
  final Future<void> Function(BuildContext context, String aid)? diverged;

  /// The version Claude's reply on thread `tid` of app `aid` waits for, or
  /// null when none waits.
  final Future<int?> Function(String aid, String tid)? heldReply;

  /// Why the publish of v`version` of app `aid` did not land, when a
  /// publish left to go on ended refused or moved; else null.
  final Future<String?> Function(String aid, int version)? heldNote;

  /// Publish vN and reply on thread `tid` of app `aid`: null once done,
  /// else why not in a sentence.
  final Future<String?> Function(String aid, String tid, int version)?
  publishAndReply;
}
