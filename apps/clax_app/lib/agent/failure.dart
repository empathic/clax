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

/// One file a stopped change's kept draft changed, with the lines it
/// added and removed.
typedef FileChange = ({String path, int added, int removed});

/// At most this many files are named in a limit stop's reply.
const maxReplyFiles = 5;

/// At most this many of the agent's notes are quoted in a limit stop's
/// reply: the latest.
const maxReplyNotes = 3;

/// How far a change that stopped at a limit has got, as its reply says:
/// how many of its runs stopped ([stops], this one included), what they
/// cost together ([spentUsd] of [changeCapUsd]), the agent's notes over
/// those runs, oldest first, and the files its kept draft changed.
class LimitProgress {
  const LimitProgress({
    required this.stops,
    required this.spentUsd,
    this.notes = const [],
    this.changed = const [],
  });

  final int stops;
  final double spentUsd;
  final List<String> notes;
  final List<FileChange> changed;
}

/// The paragraphs [p] adds to a limit stop's reply: "Stopped twice ·
/// $2.10 of $6 used", then "Changed so far: index.html (+42 −10),
/// app.js (+8)" (at most [maxReplyFiles] files, then "and N more"; left
/// out when the draft holds no change), then the latest [maxReplyNotes]
/// notes, one per line, a note repeated in a row quoted once.
String limitProgressText(LimitProgress p) {
  final b = StringBuffer(
    '\n\nStopped ${timesWords(p.stops)} · '
    '\$${p.spentUsd.toStringAsFixed(2)} of '
    '\$${changeCapUsd.toStringAsFixed(0)} used',
  );
  if (p.changed.isNotEmpty) {
    String counts(FileChange f) => [
      if (f.added > 0) '+${f.added}',
      if (f.removed > 0) '−${f.removed}',
    ].join(' ');
    final shown = [
      for (final f in p.changed.take(maxReplyFiles))
        switch (counts(f)) {
          '' => f.path,
          final c => '${f.path} ($c)',
        },
    ];
    final more = p.changed.length - shown.length;
    b.write(
      '\nChanged so far: ${shown.join(', ')}'
      '${more > 0 ? ', and $more more' : ''}',
    );
  }
  // A note the agent repeated turn after turn is quoted once.
  final notes = <String>[];
  for (final n in p.notes.map((n) => n.trim())) {
    if (n.isNotEmpty && (notes.isEmpty || notes.last != n)) notes.add(n);
  }
  if (notes.isNotEmpty) {
    final latest = notes.length <= maxReplyNotes
        ? notes
        : notes.sublist(notes.length - maxReplyNotes);
    b.write('\n\nThe agent\'s latest notes:');
    for (final n in latest) {
      b.write('\n- $n');
    }
  }
  return b.toString();
}

/// The reply of a change that stopped at the limit [cap] with nothing
/// published: a [failureReply] in Clax's words naming the limit, then
/// [keepGoingLine]; then, with [progress], how far it has got
/// ([limitProgressText]), else the agent's own words ([agentSaid]) as
/// detail.
String limitFailureReply(
  String cap, {
  required double leftUsd,
  String? agentSaid,
  LimitProgress? progress,
}) =>
    '${failureReply(limitFailureReason(cap, leftUsd: leftUsd))}'
    '${progress != null ? limitProgressText(progress) : _agentSaid(agentSaid)}';

/// The reason of a limit stop that published nothing, without the
/// failure prefix: what the Working chip shows.
String limitFailureReason(String cap, {required double leftUsd}) =>
    'it was too big for one go — it needed ${limitWords(cap)}. '
    '${keepGoingLine(leftUsd)}';

/// The reply of a change that stopped at the limit [cap] and published
/// the part that worked: Clax's sentence naming the limit,
/// [keepGoingLine]; then, with [progress], how far it has got
/// ([limitProgressText]), else the agent's own words ([agentSaid]).
/// [checksFailed]: the published render had failing tests.
String limitLandedReply(
  String cap, {
  required double leftUsd,
  String? agentSaid,
  bool checksFailed = false,
  LimitProgress? progress,
}) =>
    'This change was too big for one go — it needed ${limitWords(cap)} — '
    'so this version has only part of it.'
    '${checksFailed ? ' Some of its checks failed.' : ''} '
    '${keepGoingLine(leftUsd)}'
    '${progress != null ? limitProgressText(progress) : _agentSaid(agentSaid)}';

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
