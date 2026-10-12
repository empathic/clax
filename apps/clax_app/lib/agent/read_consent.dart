/// The owner's consent for the agent to read another app's documents.
///
/// This consent is the agent's alone: it is separate from an app's own
/// `db.read` declaration and the owner's grant for the page to read
/// another app at runtime, and neither implies the other.
library;

/// The owner's answer to a [AgentReadConsent.ask].
enum ConsentAnswer {
  /// Allowed for this run only.
  once,

  /// Allowed for this run and every later run on the same pair of apps.
  always,

  /// Not allowed.
  denied,
}

/// Asks the owner whether the agent may read another app's documents.
abstract interface class AgentReadConsent {
  /// Asks whether the agent, working on app [reader] ([readerTitle]; both
  /// null for a new app, which has no ID yet), may read the documents of
  /// app [source] ([sourceTitle]).
  ///
  /// An implementation remembers [ConsentAnswer.always] per ([reader],
  /// [source]) pair and answers it again without asking; a null [reader]
  /// is never remembered.
  Future<ConsentAnswer> ask({
    required String? reader,
    required String? readerTitle,
    required String source,
    required String sourceTitle,
  });
}

/// One [AgentReadConsent.ask] a [FakeReadConsent] answered.
typedef ConsentAsk = ({
  String? reader,
  String? readerTitle,
  String source,
  String sourceTitle,
  bool remembered,
});

/// An [AgentReadConsent] for tests: answers each question shown with the
/// next of [answers] (then [fallback]), or with [answer] when set, and
/// remembers [ConsentAnswer.always] per pair as the real one does.
class FakeReadConsent implements AgentReadConsent {
  FakeReadConsent({
    List<ConsentAnswer> answers = const [],
    this.fallback = ConsentAnswer.denied,
    this.answer,
  }) : _answers = List.of(answers);

  final List<ConsentAnswer> _answers;
  final ConsentAnswer fallback;

  /// When set, answers each question shown by awaiting it.
  Future<ConsentAnswer> Function(ConsentAsk ask)? answer;

  /// Every ask, in order; `remembered` ones showed no question.
  final List<ConsentAsk> asks = [];
  final Set<(String, String)> _always = {};

  /// The questions shown to the owner.
  int get shown => asks.where((a) => !a.remembered).length;

  /// Remembers `always` for [reader] reading [source], as an earlier run
  /// would have.
  void allowAlways(String reader, String source) =>
      _always.add((reader, source));

  @override
  Future<ConsentAnswer> ask({
    required String? reader,
    required String? readerTitle,
    required String source,
    required String sourceTitle,
  }) async {
    final remembered = reader != null && _always.contains((reader, source));
    final a = (
      reader: reader,
      readerTitle: readerTitle,
      source: source,
      sourceTitle: sourceTitle,
      remembered: remembered,
    );
    asks.add(a);
    if (remembered) return ConsentAnswer.always;
    final out = switch (answer) {
      final f? => await f(a),
      null => _answers.isEmpty ? fallback : _answers.removeAt(0),
    };
    if (out == ConsentAnswer.always && reader != null) {
      _always.add((reader, source));
    }
    return out;
  }
}
