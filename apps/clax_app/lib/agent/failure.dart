/// The reply the app's agent posts on a thread whose change it could not
/// make, and how the app recognises it.
///
/// Every producer of the reply writes it with [failureReply], and every
/// reader (the worker's retry, the thread page) recognises it with
/// [isAppFailureReply], so rewording it changes both together.
library;

import '../clax/models.dart' show ThreadComment;

/// How a failure reply opens.
const failurePrefix = "Couldn't make that change:";

/// The same opening with a typographic apostrophe, as a person or a
/// keyboard may have typed it.
const _curlyPrefix = 'Couldn’t make that change:';

/// The app session's agent harness (`via_harness` on its comments).
const appHarness = 'app';

/// The failure reply for [reason]: "Couldn't make that change: [reason]".
String failureReply(String reason) => '$failurePrefix $reason';

/// Whether [body] opens as a failure reply does.
bool isFailureText(String body) =>
    body.startsWith(failurePrefix) || body.startsWith(_curlyPrefix);

/// Whether [c] is a failure reply of this app's agent: an agent comment
/// posted by the app session (`via_harness` `app`) that opens as
/// [failureReply] does. A reply from another harness, or a person's
/// comment with the same words, is not one.
bool isAppFailureReply(ThreadComment c) =>
    c.byAgent && c.viaHarness == appHarness && isFailureText(c.body);

/// What a change and its Keep going runs may cost together, in US dollars.
const changeCapUsd = 6.0;

/// Less than this left of [changeCapUsd] is too little for another run.
const minKeepGoingUsd = 0.05;

/// A change that has stopped at a limit this many times is planned rather
/// than run on: its Keep going run puts what is left into plan features,
/// and the build's continue runs build them.
const planAfterStops = 2;

/// [n] times in words: "once", "twice", "3 times".
String timesWords(int n) => switch (n) {
  1 => 'once',
  2 => 'twice',
  _ => '$n times',
};

/// The line a limit stop's reply carries while Keep going can run it on;
/// the thread page and the Working chip offer Keep going on a reply that
/// has it ([offersKeepGoing]).
const keepGoingHint = 'Tap Keep going to pick up where it stopped.';

/// A hard cap as [RunBudget.hardStop] names it ("10 turns", "120 seconds",
/// "32000 output tokens", r"$1.50", or "3 renders") in the person's words:
/// "more than 10 steps", "more than 2 minutes".
String limitWords(String cap) {
  final m = RegExp(r'^(\d+) (turns|renders|seconds|output tokens)$')
      .firstMatch(cap);
  if (m == null) {
    return cap.startsWith(r'$')
        ? 'more than its $cap a run'
        : 'more than one go allows ($cap)';
  }
  final n = int.parse(m[1]!);
  return switch (m[2]) {
    'turns' => 'more than $n steps',
    'renders' => 'more than $n checks of its work',
    'seconds' when n % 60 == 0 =>
      'more than ${n ~/ 60} minute${n == 60 ? '' : 's'}',
    'seconds' => 'more than $n seconds',
    _ => 'more writing than one go allows',
  };
}

/// What a limit stop's reply says about going on, given [leftUsd] of the
/// change's [changeCapUsd]: [keepGoingHint], with what is left once that
/// is under half; or, with too little left, that it cannot go on.
String keepGoingLine(double leftUsd) {
  if (leftUsd < minKeepGoingUsd) {
    return "It has used the \$${changeCapUsd.toStringAsFixed(0)} a change "
        "may cost, so it can't keep going: reply with a smaller step.";
  }
  if (leftUsd < changeCapUsd / 2) {
    return '$keepGoingHint About \$${leftUsd.toStringAsFixed(2)} of its '
        '\$${changeCapUsd.toStringAsFixed(0)} is left.';
  }
  return keepGoingHint;
}

/// The reply of a change that stopped at the limit [cap] with nothing
/// published: a [failureReply] in Clax's words naming the limit, then
/// [keepGoingLine], then the agent's own words ([agentSaid]) as detail.
String limitFailureReply(
  String cap, {
  required double leftUsd,
  String? agentSaid,
}) =>
    '${failureReply(limitFailureReason(cap, leftUsd: leftUsd))}'
    '${_agentSaid(agentSaid)}';

/// The reason of a limit stop that published nothing, without the
/// failure prefix: what the Working chip shows.
String limitFailureReason(String cap, {required double leftUsd}) =>
    'it was too big for one go — it needed ${limitWords(cap)}. '
    '${keepGoingLine(leftUsd)}';

/// The reply of a change that stopped at the limit [cap] and published
/// the part that worked: Clax's sentence naming the limit,
/// [keepGoingLine], then the agent's own words ([agentSaid]).
/// [checksFailed]: the published render had failing tests.
String limitLandedReply(
  String cap, {
  required double leftUsd,
  String? agentSaid,
  bool checksFailed = false,
}) =>
    'This change was too big for one go — it needed ${limitWords(cap)} — '
    'so this version has only part of it.'
    '${checksFailed ? ' Some of its checks failed.' : ''} '
    '${keepGoingLine(leftUsd)}${_agentSaid(agentSaid)}';

String _agentSaid(String? words) {
  final w = (words ?? '').trim();
  return w.isEmpty ? '' : '\n\nThe agent said: $w';
}

/// Whether [text] (a reply, or the Working chip's failure text) offers
/// Keep going.
bool offersKeepGoing(String text) => text.contains(keepGoingHint);

/// Whether [c] is a limit stop's reply of this app's agent that offers
/// Keep going: an agent comment posted by the app session that carries
/// [keepGoingHint]. The change it ended may have published part of
/// itself (then it is not a failure reply) or nothing.
bool isKeepGoingReply(ThreadComment c) =>
    c.byAgent && c.viaHarness == appHarness && offersKeepGoing(c.body);
