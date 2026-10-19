/// The reply the app's agent posts on a thread whose change it could not
/// make, and how the app recognises it.
///
/// Every producer of the reply writes it with [failureReply], and every
/// reader (the worker's retry, the thread page) recognises it with
/// [isAppFailureReply], so rewording it changes both together.
library;

import '../clax/models.dart' show ThreadComment;
import '../ui/kit/verbs.dart' show ClaxVerbs;
import '../ui/user_message.dart' show userMessage;
import 'backend.dart' show AgentError;

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

/// What a Keep going run's reply opens with when it could not go on from
/// the stopped run's draft (the app changed under it, or it was not kept)
/// and started again from the app as it is.
const startedOverLine =
    "Started again from the current app: the stopped run's draft could not "
    'be used.';

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

/// The line a limit stop's reply carries while Keep going can run it on,
/// as replies are stored: [keepGoingHintFor] by touch. The thread page and
/// the Working chip offer Keep going on a reply that has it
/// ([offersKeepGoing]); the thread page shows it in the reader's words
/// ([keepGoingIn]). Stored replies keep these words, so they never change.
const keepGoingHint = 'Tap Keep going to pick up where it stopped.';

/// The Keep going line in [v]'s words: "Tap Keep going to pick up where
/// it stopped." by touch, "Click …" with a mouse.
String keepGoingHintFor(ClaxVerbs v) =>
    '${v.tap} Keep going to pick up where it stopped.';

/// The Keep going line in any pointer's words: a stored reply carries the
/// "Tap" one, and a reply shown with a mouse the "Click" one.
final _keepGoingAny = RegExp(
  '(?:${ClaxVerbs.touch.tap}|${ClaxVerbs.mouse.tap}) Keep going to pick up '
  r'where it stopped\.',
);

/// [text] with its Keep going line in [v]'s words.
String keepGoingIn(String text, ClaxVerbs v) =>
    text.replaceAll(_keepGoingAny, keepGoingHintFor(v));

/// What a limit stop's reply says once the change has spent
/// [changeCapUsd] ([keepGoingLine]).
const _spentWords = "so it can't keep going";

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
        'may cost, $_spentWords: reply with a smaller step.';
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
    this.startedOver = false,
  });

  /// This run started again from the app ([startedOverLine]).
  final bool startedOver;
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
    '\n\n${p.startedOver ? '$startedOverLine\n' : ''}'
    'Stopped ${timesWords(p.stops)} · '
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

/// The reason of a change that stopped at the limit [cap] after its run
/// planned what is left (`plan_set` with features to build), which the
/// build's runs then build: no Keep going is offered. [landed]: the cap
/// stop published part of it.
String plannedStopReason(String cap, {bool landed = false}) =>
    'This change was too big for one go — it needed ${limitWords(cap)} — '
    '${landed ? 'so this version has part of it, and the rest is' : 'so what is left of it is'} '
    'planned and being built in steps: the plan shows each one as it is '
    'built.';

/// The reply of such a change ([plannedStopReason]), with how far it got
/// ([limitProgressText]); [checksFailed]: the published render had failing
/// tests.
String plannedStopReply(
  String cap, {
  bool landed = false,
  bool checksFailed = false,
  required LimitProgress progress,
}) =>
    '${plannedStopReason(cap, landed: landed)}'
    '${checksFailed ? ' Some of its checks failed.' : ''}'
    '${limitProgressText(progress)}';

/// The stop count and spend a limit reply states ([limitProgressText]:
/// "Stopped twice · $2.10 of $6 used"), or null when [body] states none:
/// what a Keep going with nothing kept goes on counting from.
({int stops, double spentUsd})? parseLimitProgress(String body) {
  final m = RegExp(
    r'(?:^|\n)Stopped (once|twice|(\d+) times) · \$(\d+\.\d\d) of \$\d+ used',
  ).firstMatch(body);
  if (m == null) return null;
  final stops = switch (m[1]) {
    'once' => 1,
    'twice' => 2,
    _ => int.parse(m[2]!),
  };
  return (stops: stops, spentUsd: double.parse(m[3]!));
}

/// Whether [text] (a reply, or the Working chip's failure text) offers
/// Keep going: it carries the Keep going line in either pointer's words,
/// the stored "Tap" sentence or [keepGoingHintFor]'s.
bool offersKeepGoing(String text) => _keepGoingAny.hasMatch(text);

/// Whether [c] is a limit stop's reply of this app's agent that offers
/// Keep going: an agent comment posted by the app session that carries
/// the Keep going line ([offersKeepGoing]). The change it ended may have published part of
/// itself (then it is not a failure reply) or nothing.
bool isKeepGoingReply(ThreadComment c) =>
    c.byAgent && c.viaHarness == appHarness && offersKeepGoing(c.body);

/// An app agent's reply read back for the thread page: one [line] to
/// show, and what goes under Details.
final class FailureReply {
  const FailureReply({
    required this.line,
    this.stops,
    this.costUsd,
    this.capUsd,
    this.files = const [],
    this.moreFiles = 0,
    this.notes = const [],
    this.raw,
    this.saysSpent = false,
  });

  /// The reply's first paragraph without the failure prefix, as a
  /// sentence; a raw error's [userMessage].
  final String line;

  /// How often the change stopped at a limit, this stop included.
  final int? stops;

  /// What the change's runs cost together, of its [capUsd].
  final double? costUsd, capUsd;

  /// The files the change's draft changed, with their lines.
  final List<FileChange> files;

  /// How many more files changed than [files] names.
  final int moreFiles;

  /// The agent's notes and anything else the reply says, oldest first;
  /// each may be Markdown.
  final List<String> notes;

  /// The reason as written when [line] is its [userMessage] instead, for
  /// Developer tools.
  final String? raw;

  /// The reply says the change has spent [changeCapUsd].
  final bool saysSpent;

  /// The change has spent its limit: under [minKeepGoingUsd] of [capUsd]
  /// is left, or the reply says so. Retry would run nothing.
  bool get spentLimit =>
      saysSpent ||
      (costUsd != null && capUsd != null && capUsd! - costUsd! < minKeepGoingUsd);

  /// Whether Details has anything to hold besides [raw].
  bool get hasDetails =>
      costUsd != null || files.isNotEmpty || notes.isNotEmpty;
}

final _stoppedLine = RegExp(
  r'^Stopped (once|twice|(\d+) times) · \$(\d+\.\d\d) of \$(\d+) used$',
);
final _fileItem = RegExp(r'^(.+?)(?: \((?:\+(\d+))? ?(?:−(\d+))?\))?$');
final _moreItem = RegExp(r'^and (\d+) more$');
const _notesHeading = "The agent's latest notes:";
const _agentSaidLead = 'The agent said: ';

/// [body], a reply of this app's agent, read back: what
/// [limitFailureReply], [limitLandedReply], [plannedStopReply] and a
/// [failureReply] write. The first paragraph is the [FailureReply.line]
/// (a failure's reason as a sentence, through [userMessage]); the
/// progress paragraph gives the stops, the cost and the files; the notes,
/// the agent's words and any other paragraph are notes.
FailureReply parseFailureReply(String body) {
  var text = body.trim();
  final failed = isFailureText(text);
  if (failed) text = text.substring(failurePrefix.length).trim();
  final paras = text.split(RegExp(r'\n[ \t]*\n'));
  final head = paras.first.trim();
  var line = head;
  String? raw;
  if (failed) {
    line = userMessage(AgentError(head));
    if (line != head) raw = head;
    line = line.isEmpty ? line : '${line[0].toUpperCase()}${line.substring(1)}';
  }
  int? stops;
  double? cost, cap;
  final files = <FileChange>[];
  var more = 0;
  final notes = <String>[];
  for (final p in paras.skip(1)) {
    final lines = p.trim().split('\n');
    if (lines.first.trim() == _notesHeading) {
      for (final l in lines.skip(1)) {
        final n = l.trim();
        notes.add(n.startsWith('- ') ? n.substring(2).trim() : n);
      }
      continue;
    }
    if (p.trim().startsWith(_agentSaidLead)) {
      notes.add(p.trim().substring(_agentSaidLead.length).trim());
      continue;
    }
    final other = <String>[];
    for (final l in lines.map((l) => l.trim())) {
      final m = _stoppedLine.firstMatch(l);
      if (m != null) {
        stops = switch (m[1]) {
          'once' => 1,
          'twice' => 2,
          _ => int.parse(m[2]!),
        };
        cost = double.parse(m[3]!);
        cap = double.parse(m[4]!);
      } else if (l.startsWith('Changed so far: ')) {
        for (final item in l.substring('Changed so far: '.length).split(', ')) {
          final rest = _moreItem.firstMatch(item);
          if (rest != null) {
            more = int.parse(rest[1]!);
            continue;
          }
          final f = _fileItem.firstMatch(item)!;
          files.add((
            path: f[1]!,
            added: int.tryParse(f[2] ?? '') ?? 0,
            removed: int.tryParse(f[3] ?? '') ?? 0,
          ));
        }
      } else if (l.isNotEmpty) {
        other.add(l);
      }
    }
    if (other.isNotEmpty) notes.add(other.join('\n'));
  }
  return FailureReply(
    line: line,
    stops: stops,
    costUsd: cost,
    capUsd: cap,
    files: files,
    moreFiles: more,
    notes: notes,
    raw: raw,
    saysSpent: head.contains(_spentWords),
  );
}
