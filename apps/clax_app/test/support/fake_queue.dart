import 'package:clax_app/queue/client.dart';
import 'package:flutter/foundation.dart';

/// A [QueueClient] a test drives: it [push]es the states the worker would
/// publish and reads the intents the UI [sent]. It runs nothing: an
/// intent changes [state] only when the test says so.
class FakeQueueClient implements QueueClient {
  FakeQueueClient([QueueState initial = const QueueState([])])
    : _state = ValueNotifier(initial);

  final ValueNotifier<QueueState> _state;

  /// Every intent sent, oldest first.
  final sent = <Intent>[];

  @override
  ValueListenable<QueueState> get state => _state;

  @override
  void send(Intent intent) => sent.add(intent);

  /// Publishes [jobs] as the queue's state; [loaded] false as a worker
  /// publishes it before it has loaded the core's jobs.
  void push(List<JobView> jobs, {bool loaded = true}) =>
      _state.value = QueueState(jobs, loaded: loaded);

  /// Publishes the state with job [view] in place of the one of its ID,
  /// or added after the others.
  void put(JobView view) {
    final jobs = [..._state.value.jobs];
    final i = jobs.indexWhere((j) => j.id == view.id);
    if (i < 0) {
      jobs.add(view);
    } else {
      jobs[i] = view;
    }
    push(jobs);
  }

  /// The intents of type [T] sent, oldest first.
  List<T> sentOf<T extends Intent>() => sent.whereType<T>().toList();
}

/// A make's view for tests: [state], with what that state offers unless
/// [offers] says otherwise.
JobView makeJob(
  String id, {
  JobState state = JobState.running,
  String title = 'a timer',
  String? artifactId,
  WaitingFor? waitingFor,
  String? error,
  Set<Offer>? offers,
}) => JobView(
  id: id,
  kind: JobKind.make,
  state: state,
  title: title,
  artifactId: artifactId,
  waitingFor: waitingFor,
  error: error,
  offers: offers ?? _offers(state),
);

/// A change's view of app [artifactId] for tests.
JobView changeJob(
  String id,
  String artifactId, {
  JobState state = JobState.running,
  WaitingFor? waitingFor,
  String? error,
  Set<Offer>? offers,
}) => JobView(
  id: id,
  kind: JobKind.change,
  state: state,
  artifactId: artifactId,
  waitingFor: waitingFor,
  error: error,
  offers: offers ?? _offers(state),
);

Set<Offer> _offers(JobState s) => switch (s) {
  JobState.queued ||
  JobState.running ||
  JobState.waiting ||
  JobState.paused => const {Offer.cancel},
  JobState.stopped => const {Offer.keepGoing, Offer.discard},
  JobState.failed => const {Offer.retry, Offer.discard},
  JobState.cancelled || JobState.done => const {},
};
