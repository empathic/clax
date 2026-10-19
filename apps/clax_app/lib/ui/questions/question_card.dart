import 'package:flutter/material.dart';

import '../../clax/models.dart';
import '../kit/kit.dart';
import '../theme.dart';
import '../tokens.g.dart';

/// One of Clax's question sets ([question]), answered here: each of its
/// one to four questions as its header and text, then its options as rows
/// with a label and a detail (radios for a single choice, checkboxes for a
/// multiple one), "Other…" with a field where the question takes one, or a
/// field alone for a free-text question; then Skip and **Answer**.
///
/// Its byline is "Clax", with "In a thread" for a question asked in one;
/// never the asking session's name or its folder. Answer is enabled once
/// every question has an answer, and calls [onAnswer] with one
/// [QuestionAnswer] per question, in order; Skip calls [onSkip]. While
/// either runs ([busy]) both are disabled.
class QuestionCard extends StatefulWidget {
  const QuestionCard({
    super.key,
    required this.question,
    required this.onAnswer,
    required this.onSkip,
    this.busy = false,
    this.error,
  });

  final QuestionView question;
  final ValueChanged<List<QuestionAnswer>> onAnswer;
  final VoidCallback onSkip;
  final bool busy;

  /// A line under the card saying the last answer did not go.
  final String? error;

  @override
  State<QuestionCard> createState() => _QuestionCardState();
}

/// What the person has given for one question: the labels picked, whether
/// "Other…" is picked, and its text.
class _Choice {
  final picked = <String>[];
  var other = false;
  final text = TextEditingController();
}

class _QuestionCardState extends State<QuestionCard> {
  late final List<_Choice> _choices = [
    for (final _ in widget.question.questions) _Choice(),
  ];

  @override
  void dispose() {
    for (final c in _choices) {
      c.text.dispose();
    }
    super.dispose();
  }

  /// The answer to [q] as given in [c], or null while it has none.
  static QuestionAnswer? _answerOf(AskedQuestion q, _Choice c) {
    final typed = c.text.text.trim();
    if (q.options.isEmpty) {
      return typed.isEmpty ? null : QuestionAnswer(text: typed);
    }
    final text = c.other && typed.isNotEmpty ? typed : null;
    if (c.picked.isEmpty && text == null) return null;
    if (c.other && text == null) return null;
    final picked = [
      for (final o in q.options)
        if (c.picked.contains(o.label)) o.label,
    ];
    return QuestionAnswer(selected: picked, text: text);
  }

  List<QuestionAnswer>? get _answers {
    final out = <QuestionAnswer>[];
    final qs = widget.question.questions;
    for (var i = 0; i < qs.length; i++) {
      final a = _answerOf(qs[i], _choices[i]);
      if (a == null) return null;
      out.add(a);
    }
    return out;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final answers = _answers;
    final byline = widget.question.threadId == null
        ? 'Clax'
        : 'Clax · In a thread';
    final small = apple
        ? ClaxType.of(ClaxRole.callout, theme.platform)
        : theme.textTheme.bodyMedium!;
    final qs = widget.question.questions;
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(byline, style: small.copyWith(color: p.agentInk)),
        for (var i = 0; i < qs.length; i++) ...[
          const SizedBox(height: ClaxSpace.s8),
          _question(context, qs[i], _choices[i]),
        ],
        if (widget.error case final error?) ...[
          const SizedBox(height: ClaxSpace.s8),
          Text(error, style: small.copyWith(color: p.danger)),
        ],
        const SizedBox(height: ClaxSpace.s12),
        ClaxActionBar(
          children: [
            ClaxButton(
              label: 'Skip',
              onPressed: widget.busy ? null : widget.onSkip,
            ),
            ClaxButton(
              label: 'Answer',
              kind: ClaxButtonKind.primary,
              isDefault: true,
              onPressed: widget.busy || answers == null
                  ? null
                  : () => widget.onAnswer(answers),
            ),
          ],
        ),
      ],
    );
  }

  Widget _question(BuildContext context, AskedQuestion q, _Choice c) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final apple = ClaxFamily.of(context) == ClaxFamily.apple;
    final enabled = !widget.busy;
    final heading = apple
        ? ClaxType.of(ClaxRole.headline, theme.platform)
        : theme.textTheme.titleMedium!;
    final header = apple
        ? ClaxType.of(ClaxRole.caption, theme.platform)
        : theme.textTheme.labelMedium!;
    final field = ClaxTextField(
      controller: c.text,
      label: q.options.isEmpty ? 'Your answer' : 'Other',
      maxLines: null,
      onChanged: (_) => setState(() {}),
    );
    // A single choice keeps one pick: an option, or Other.
    const otherKey = '\u0000other';
    final group = c.other ? otherKey : c.picked.firstOrNull;
    void pick(String label) => setState(() {
      c.other = false;
      c.picked
        ..clear()
        ..add(label);
    });
    return Column(
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        if (q.header.isNotEmpty)
          Text(q.header, style: header.copyWith(color: p.muted)),
        Text(q.question, style: heading.copyWith(color: p.fg)),
        const SizedBox(height: ClaxSpace.s4),
        for (final o in q.options)
          if (q.multiSelect)
            ClaxCheckbox(
              value: c.picked.contains(o.label),
              label: o.label,
              detail: _detail(o),
              onChanged: enabled
                  ? (on) => setState(() {
                      on ? c.picked.add(o.label) : c.picked.remove(o.label);
                    })
                  : null,
            )
          else
            ClaxRadio<String>(
              value: o.label,
              group: group,
              label: o.label,
              detail: _detail(o),
              onChanged: enabled ? pick : null,
            ),
        if (q.options.isNotEmpty && q.other)
          if (q.multiSelect)
            ClaxCheckbox(
              value: c.other,
              label: 'Other…',
              onChanged: enabled ? (on) => setState(() => c.other = on) : null,
            )
          else
            ClaxRadio<String>(
              value: otherKey,
              group: group,
              label: 'Other…',
              onChanged: enabled
                  ? (_) => setState(() {
                      c.picked.clear();
                      c.other = true;
                    })
                  : null,
            ),
        if (q.options.isEmpty || c.other) ...[
          const SizedBox(height: ClaxSpace.s8),
          field,
        ],
      ],
    );
  }

  /// An option's second line: its description, and "Recommended" for the
  /// option Clax recommends.
  static String? _detail(QuestionOption o) {
    final d = o.description?.trim() ?? '';
    if (o.recommended) return d.isEmpty ? 'Recommended' : '$d · Recommended';
    return d.isEmpty ? null : d;
  }
}
