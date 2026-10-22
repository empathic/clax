import 'dart:async';

import 'package:flutter/material.dart';

import '../../clax/api.dart';
import '../kit/kit.dart';
import '../theme.dart';
import '../tokens.g.dart';
import '../user_message.dart';
import 'question_card.dart';

/// Clax's open questions about one app, answered natively: read from the
/// daemon and answered through it, never through the shell.
abstract final class QuestionsSheet {
  /// Shows the open questions about [artifactId] (those about no app when
  /// it is null), and question [focus] whatever it is about, each in a
  /// [QuestionCard], the one [focus] names first,
  /// under the asking mark and "Clax has a question" (or "Clax has N
  /// questions"). A question asked in a thread of the app is one of them.
  ///
  /// It reads [ClaxApi.questions] as it opens and again on each event of
  /// [changes], keeping what is chosen in the cards still open. A card
  /// answered ([ClaxApi.answerQuestion]) or skipped
  /// ([ClaxApi.declineQuestion]) leaves, as does one the daemon answers
  /// with 409 `question_closed`; the sheet closes once no card is left,
  /// and at once when nothing was open. It completes as it closes.
  /// [onShown] is called with each question's ID as its card first shows.
  ///
  /// A [ClaxSheet]: at compact width a bottom sheet with a handle, from
  /// medium a centred dialog, and the Apple family's 560 sheet from the top
  /// of the window.
  static Future<void> show(
    BuildContext context, {
    required ClaxApi api,
    required String? artifactId,
    String? focus,
    Stream<void>? changes,
    ValueChanged<String>? onShown,
  }) => ClaxSheet.show<void>(
    context,
    builder: (_) => QuestionsSheetView(
      api: api,
      artifactId: artifactId,
      focus: focus,
      changes: changes,
      onShown: onShown,
    ),
  );
}

/// The questions sheet's content ([QuestionsSheet.show]); it pops its route
/// once no question is left.
class QuestionsSheetView extends StatefulWidget {
  const QuestionsSheetView({
    super.key,
    required this.api,
    required this.artifactId,
    this.focus,
    this.changes,
    this.onShown,
  });

  final ClaxApi api;
  final String? artifactId;
  final String? focus;
  final Stream<void>? changes;
  final ValueChanged<String>? onShown;

  @override
  State<QuestionsSheetView> createState() => _QuestionsSheetViewState();
}

class _QuestionsSheetViewState extends State<QuestionsSheetView> {
  /// The cards, in order; null until the first read.
  List<QuestionView>? _list;

  /// The questions answered or skipped here, which a read under way may
  /// still list.
  final _gone = <String>{};
  final _shown = <String>{};
  final _busy = <String>{};
  final _errors = <String, String>{};
  String? _loadError;
  StreamSubscription<void>? _sub;
  var _closed = false;

  @override
  void initState() {
    super.initState();
    unawaited(_read());
    _sub = widget.changes?.listen((_) => unawaited(_read()));
  }

  @override
  void dispose() {
    unawaited(_sub?.cancel());
    super.dispose();
  }

  Future<void> _read() async {
    final List<QuestionView> open;
    try {
      open = await widget.api.questions();
    } on Object catch (e) {
      if (!mounted) return;
      setState(() => _loadError = logUserMessage('questions', e));
      return;
    }
    if (!mounted) return;
    final mine = [
      for (final q in open)
        if (q.isOpen &&
            (q.artifactId == widget.artifactId || q.id == widget.focus) &&
            !_gone.contains(q.id))
          q,
    ];
    final focus = widget.focus;
    final first = mine.indexWhere((q) => q.id == focus);
    if (first > 0) mine.insert(0, mine.removeAt(first));
    setState(() {
      _loadError = null;
      _list = mine;
    });
    for (final q in mine) {
      if (_shown.add(q.id)) widget.onShown?.call(q.id);
    }
    _closeWhenEmpty();
  }

  void _drop(String qid) {
    _gone.add(qid);
    setState(() {
      _list?.removeWhere((q) => q.id == qid);
      _busy.remove(qid);
      _errors.remove(qid);
    });
    _closeWhenEmpty();
  }

  void _closeWhenEmpty() {
    if (_closed || !mounted || (_list?.isNotEmpty ?? true)) return;
    _closed = true;
    final route = ModalRoute.of(context);
    if (route == null) return;
    if (route.isCurrent) {
      Navigator.of(context).pop();
    } else if (route.isActive) {
      Navigator.of(context).removeRoute(route);
    }
  }

  /// Runs [send] for question [qid]: done or 409 `question_closed`, the
  /// card leaves; any other failure says so on the card.
  Future<void> _send(String qid, Future<void> Function() send) async {
    setState(() {
      _busy.add(qid);
      _errors.remove(qid);
    });
    try {
      await send();
    } on ClaxApiError catch (e) {
      if (!mounted) return;
      if (e.status == 409 && e.code == 'question_closed') {
        _drop(qid);
      } else {
        _failed(qid, e);
      }
      return;
    } on Object catch (e) {
      if (mounted) _failed(qid, e);
      return;
    }
    if (mounted) _drop(qid);
  }

  void _failed(String qid, Object e) => setState(() {
    _busy.remove(qid);
    _errors[qid] = logUserMessage('answer', e);
  });

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final list = _list;
    final n = list?.length ?? 0;
    final title = n > 1 ? 'Clax has $n questions' : 'Clax has a question';
    final titleStyle = apple
        ? ClaxType.of(ClaxRole.headline, theme.platform)
        : theme.textTheme.titleLarge!;
    final Widget body;
    if (list == null) {
      body = _loadError == null
          ? const Padding(
              padding: EdgeInsets.all(ClaxSpace.s24),
              child: Center(child: CircularProgressIndicator()),
            )
          : _error(context);
    } else {
      body = SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            for (final (i, q) in list.indexed) ...[
              if (i > 0)
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: ClaxSpace.s16),
                  child: Divider(height: 1, color: p.border),
                ),
              QuestionCard(
                key: ValueKey(q.id),
                question: q,
                busy: _busy.contains(q.id),
                error: _errors[q.id],
                onAnswer: (answers) => unawaited(
                  _send(
                    q.id,
                    () => widget.api.answerQuestion(q.id, [
                      for (final a in answers) a.toJson(),
                    ]),
                  ),
                ),
                onSkip: () => unawaited(
                  _send(q.id, () => widget.api.declineQuestion(q.id)),
                ),
              ),
            ],
          ],
        ),
      );
    }
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Row(
          children: [
            ExcludeSemantics(
              child: ClaxMark(size: 28, state: ClaxMarkState.asking),
            ),
            const SizedBox(width: ClaxSpace.s12),
            Expanded(
              child: Semantics(
                header: true,
                child: Text(title, style: titleStyle.copyWith(color: p.fg)),
              ),
            ),
          ],
        ),
        const SizedBox(height: ClaxSpace.s16),
        Flexible(child: body),
      ],
    );
  }

  Widget _error(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'Couldn’t show Clax’s questions. ${_loadError!}',
          style: theme.textTheme.bodyMedium!.copyWith(color: p.muted),
        ),
        const SizedBox(height: ClaxSpace.s12),
        ClaxActionBar(
          children: [
            ClaxButton(
              label: 'Try again',
              kind: ClaxButtonKind.primary,
              onPressed: () {
                setState(() => _loadError = null);
                unawaited(_read());
              },
            ),
          ],
        ),
      ],
    );
  }
}
