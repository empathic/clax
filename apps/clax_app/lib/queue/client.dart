/// What the UI isolate knows of the queue (spec
/// 2026-10-19-in-process-core-design §7.2): a snapshot of every job, and a
/// way to say what the person wants. Nothing the UI does awaits a job.
library;

import 'dart:convert';
import 'dart:math';

import 'package:flutter/foundation.dart';

import '../bundle/content_hash.dart' show sha256Hex;
import 'model.dart';
import 'msg.dart';

export 'model.dart'
    show
        JobId,
        JobKind,
        JobState,
        JobView,
        Offer,
        QueueState,
        StepKey,
        WaitingFor;
export 'msg.dart'
    show Cancel, Change, Discard, Intent, KeepGoing, Make, Retry;

/// The queue as the UI sees it: [state], rebuilt after every change of
/// any job, and [send], which hands an intent over and returns at once.
/// A job's progress shows only as a later [state].
abstract interface class QueueClient {
  /// Every job the queue holds, the core's rows and those whose creation
  /// is in flight (as `queued`).
  ValueListenable<QueueState> get state;

  /// Hands [intent] to the queue. An intent the job's row does not offer
  /// (a Retry of a running job, a Keep going without budget) changes
  /// nothing.
  void send(Intent intent);
}

/// A new job's ID for [request]: the first 24 hex digits of the SHA-256
/// of the request and a nonce from [random], so the core answers a
/// repeated creation of the same job once and two makes of the same words
/// are two jobs.
JobId newJobId(String request, {Random? random}) {
  final r = random ?? Random.secure();
  final nonce = [for (var i = 0; i < 16; i++) r.nextInt(256)];
  return sha256Hex([...utf8.encode(request), ...nonce]).substring(0, 24);
}

/// Finding jobs in a [QueueState].
extension QueueStateJobs on QueueState {
  /// Job [id], while the queue holds it.
  JobView? job(JobId id) {
    for (final j in jobs) {
      if (j.id == id) return j;
    }
    return null;
  }

  /// The makes, newest first.
  Iterable<JobView> get makes =>
      jobs.reversed.where((j) => j.kind == JobKind.make);

  /// The change of app [artifactId] the person sees: the newest one not
  /// done or cancelled, else none.
  JobView? changeOf(String artifactId) {
    for (final j in jobs.reversed) {
      if (j.kind == JobKind.change &&
          j.artifactId == artifactId &&
          !j.state.isFinal) {
        return j;
      }
    }
    return null;
  }
}
