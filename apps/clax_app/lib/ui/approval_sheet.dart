/// The sheet that asks the person whether Clax may read comments someone
/// they have not approved wrote: an approval request from their inbox.
library;

import 'package:flutter/material.dart';

import '../clax/api.dart';
import 'kit/kit.dart';
import 'theme.dart';
import 'tokens.g.dart';

/// The sheet's title: [who] (as comments name them, `Bob (on clax.dev)`)
/// wants to reach Clax.
String approvalTitle(String who) => '$who wants to reach Clax';

/// How much waits on [app] (its title, or null when unknown): `1 comment
/// on Plant care waits for your approval`.
String approvalWhat(ApprovalRequest r, String? app) {
  final n = r.count;
  final what = r.threadId == null
      ? (n == 1 ? '1 item' : '$n items')
      : (n == 1 ? '1 comment' : '$n comments');
  final on = app == null || app.trim().isEmpty ? 'one app' : app.trim();
  return n == 1
      ? '$what on $on waits for your approval.'
      : '$what on $on wait for your approval.';
}

/// A short, stable form of [p]'s public ID that tells apart people who
/// chose alike names: its first eight characters (for an author on a
/// public Clax, `remote:<origin>#<ID>`, of the ID after `#`); null when
/// [p] has none.
String? approvalHandle(CardPerson p) {
  final id = p.publicId;
  if (id == null || id.isEmpty) return null;
  final own = id.substring(id.lastIndexOf('#') + 1);
  return own.length <= 8 ? own : own.substring(0, 8);
}

/// Why it asks: what someone writes can sway what Claude does.
String approvalAside(String who) =>
    'Clax keeps what $who wrote from Claude until you allow it. What '
    'someone writes can sway what Claude does: allow only people you trust.';

/// The line a decided request reads instead of its choices.
String approvalOutcome(ApprovalRequest r) => switch (r.status) {
  'approved' => switch (r.scope) {
    'thread' => 'Allowed in this thread',
    'artifact' => 'Allowed on this app',
    'everywhere' => 'Allowed everywhere',
    _ => 'Allowed',
  },
  'declined' => 'Declined',
  'withdrawn' => 'Withdrawn: nothing waits on it any more',
  _ => 'This request no longer waits for you',
};

/// Shows request [r] (from an inbox item about [app]) in a [ClaxSheet]:
/// who wants to reach Clax, what waits, the withheld comments themselves
/// (every word: nothing withheld is hidden from the person), and the four
/// choices: this thread (including later replies; only for a request in
/// a thread), always on this app, always everywhere, or decline. Each
/// decides through [api]; completes with the request as decided, or null
/// when closed without a decision.
Future<ApprovalRequest?> showApprovalSheet(
  BuildContext context, {
  required ClaxApi api,
  required ApprovalRequest r,
  String? app,
}) => ClaxSheet.show<ApprovalRequest>(
  context,
  title: approvalTitle(r.author.name),
  builder: (_) => ApprovalBody(api: api, request: r, app: app),
);

/// The sheet's content: what waits, the comments, the aside, then the
/// choices (or the outcome of a decided request).
class ApprovalBody extends StatefulWidget {
  const ApprovalBody({
    super.key,
    required this.api,
    required this.request,
    this.app,
  });

  final ClaxApi api;
  final ApprovalRequest request;
  final String? app;

  @override
  State<ApprovalBody> createState() => _ApprovalBodyState();
}

class _ApprovalBodyState extends State<ApprovalBody> {
  var _busy = false;
  String? _error;

  /// The request as last decided here, else as the inbox gave it.
  late ApprovalRequest _r = widget.request;

  Future<void> _decide(String scope) async {
    if (_busy) return;
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      final decided = await widget.api.decideApproval(_r.id, scope);
      if (!mounted) return;
      Navigator.of(context).pop(decided);
    } on ClaxApiError catch (e) {
      if (!mounted) return;
      setState(() {
        _busy = false;
        if (e.code == 'request_closed') {
          _r = ApprovalRequest(
            id: _r.id,
            author: _r.author,
            artifactId: _r.artifactId,
            threadId: _r.threadId,
            count: _r.count,
            status: 'gone',
            createdAt: _r.createdAt,
            comments: _r.comments,
          );
        } else {
          _error = 'Clax couldn’t save that. Try again.';
        }
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final r = _r;
    final who = r.author.name;
    final handle = approvalHandle(r.author);
    return SingleChildScrollView(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            approvalWhat(r, widget.app),
            style: theme.textTheme.bodyLarge,
          ),
          if (handle != null) ...[
            const SizedBox(height: ClaxSpace.s4),
            Text(
              '$who’s ID: $handle',
              key: const ValueKey('approval-handle'),
              style: theme.textTheme.bodyMedium?.copyWith(color: p.muted),
            ),
          ],
          if (r.comments.isNotEmpty) ...[
            const SizedBox(height: ClaxSpace.s12),
            DecoratedBox(
              key: const ValueKey('approval-held'),
              decoration: BoxDecoration(
                color: p.bg,
                border: Border.all(color: p.border),
                borderRadius: BorderRadius.circular(ClaxRadius.md),
              ),
              child: Padding(
                padding: const EdgeInsets.all(ClaxSpace.s12),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    for (final (i, c) in r.comments.indexed) ...[
                      if (i > 0) const SizedBox(height: ClaxSpace.s8),
                      Text.rich(
                        TextSpan(
                          children: [
                            TextSpan(
                              text: '${c.byAgent ? '${c.authorName ?? 'An agent'} ($who’s agent)' : who}: ',
                              style: const TextStyle(
                                fontWeight: FontWeight.w600,
                              ),
                            ),
                            TextSpan(text: c.body),
                          ],
                        ),
                        style: theme.textTheme.bodyMedium?.copyWith(
                          color: p.fg,
                        ),
                      ),
                    ],
                  ],
                ),
              ),
            ),
          ],
          const SizedBox(height: ClaxSpace.s16),
          Row(
            children: [
              const ExcludeSemantics(child: ClaxMark(size: 22)),
              const SizedBox(width: ClaxSpace.s12),
              Expanded(
                child: Text(
                  approvalAside(who),
                  style: theme.textTheme.bodyMedium?.copyWith(color: p.muted),
                ),
              ),
            ],
          ),
          const SizedBox(height: ClaxSpace.s16),
          if (r.isOpen) ...[
            if (r.threadId != null)
              ClaxButton(
                key: const ValueKey('approval-thread'),
                label: 'Allow in this thread, including later replies',
                kind: ClaxButtonKind.primary,
                onPressed: _busy ? null : () => _decide('thread'),
              ),
            const SizedBox(height: ClaxSpace.s8),
            ClaxButton(
              key: const ValueKey('approval-artifact'),
              label: 'Always allow $who on this app',
              onPressed: _busy ? null : () => _decide('artifact'),
            ),
            const SizedBox(height: ClaxSpace.s8),
            ClaxButton(
              key: const ValueKey('approval-everywhere'),
              label: handle == null
                  ? 'Always allow $who everywhere'
                  : 'Always allow $who (ID $handle) everywhere',
              onPressed: _busy ? null : () => _decide('everywhere'),
            ),
            const SizedBox(height: ClaxSpace.s8),
            ClaxButton(
              key: const ValueKey('approval-decline'),
              label: 'Decline',
              onPressed: _busy ? null : () => _decide('decline'),
            ),
          ] else
            Semantics(
              liveRegion: true,
              child: Text(
                approvalOutcome(r),
                key: const ValueKey('approval-outcome'),
                style: theme.textTheme.bodyLarge?.copyWith(
                  fontWeight: FontWeight.w600,
                ),
              ),
            ),
          if (_error case final e?) ...[
            const SizedBox(height: ClaxSpace.s8),
            Text(
              e,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: theme.colorScheme.error,
              ),
            ),
          ],
        ],
      ),
    );
  }
}
