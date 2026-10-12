import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../agent/failure.dart';
import '../clax/api.dart';
import '../platform/voice.dart';

/// Where a thread stands, as the thread page shows it.
enum ThreadStatus {
  /// Open and never sent to the agent: only the owner sees it.
  unsent,

  /// Sent; the agent has not taken it yet.
  sent,

  /// The agent took it and has not answered the last comment.
  working,

  /// The agent answered the last comment.
  replied,

  /// The agent's last comment is this app's failure reply
  /// ([isAppFailureReply]).
  failed,

  /// Resolved.
  resolved,
}

/// [t]'s status. An agent that took a sent thread acknowledges its
/// feedback (the app's session acknowledges when its poll takes it), so a
/// thread whose last comment is the person's reads `working` once its
/// feedback is `acknowledged`.
ThreadStatus threadStatus(ThreadView t) {
  if (!t.isOpen) return ThreadStatus.resolved;
  if (!t.sentToAgent) return ThreadStatus.unsent;
  final last = t.comments.isEmpty ? null : t.comments.last;
  if (last != null && last.byAgent) {
    return isAppFailureReply(last) ? ThreadStatus.failed : ThreadStatus.replied;
  }
  return t.feedbackState?.state == 'acknowledged'
      ? ThreadStatus.working
      : ThreadStatus.sent;
}

/// What [anchor] is about, in a person's words, for a thread the shell
/// gave no label for: "Whole app", the quoted text, "this area" or "this
/// spot". The shell's own label (`ThreadSummary.label`) says more and is
/// preferred.
String anchorWords(Map<String, Object?> anchor) {
  if (anchor['kind'] == 'artifact') return 'Whole app';
  final quote = anchor['quote'];
  if (quote is String) {
    final q = quote.replaceAll(RegExp(r'\s+'), ' ').trim();
    if (q.isNotEmpty) {
      final chars = q.runes.toList();
      return chars.length > 60
          ? '“${String.fromCharCodes(chars.take(60)).trimRight()}…”'
          : '“$q”';
    }
  }
  return anchor['kind'] == 'area' ? 'this area' : 'this spot';
}

/// [s] with its first letter capitalised.
String _sentence(String s) =>
    s.isEmpty ? s : '${s[0].toUpperCase()}${s.substring(1)}';

const _months = [
  'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
  'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
];

/// [iso] relative to [now]: "just now", "5 min ago", "3 h ago", "2 d ago",
/// else the day ("12 Oct"); empty for a stamp it cannot read.
String relativeTime(String iso, DateTime now) {
  final t = DateTime.tryParse(iso);
  if (t == null) return '';
  final d = now.difference(t);
  if (d.inMinutes < 1) return 'just now';
  if (d.inMinutes < 60) return '${d.inMinutes} min ago';
  if (d.inHours < 24) return '${d.inHours} h ago';
  if (d.inDays < 7) return '${d.inDays} d ago';
  final l = t.toLocal();
  return '${l.day} ${_months[l.month - 1]}';
}

/// One thread of an app, whole: what it is anchored to, its clip, its
/// status, the versions that addressed it, and every comment (the
/// person's and the agent's, marked "Clax"), with a reply box.
///
/// - **Reply.** Send posts the text as the owner's comment
///   ([ClaxApi.postOwnerComment]); on a thread sent to the agent the
///   daemon sends it on. A failed post keeps the text and says why.
///   Focusing the box starts dictation when [autoDictate] is on, once per
///   focus; the mic starts and stops it. Heard words replace what
///   dictation added after the text the box held when it started.
/// - **Send to Clax** on a thread never sent ([ClaxApi.sendThread]).
/// - **Retry** when the agent's last comment is a failure reply and
///   [onRetry] is given: it calls [onRetry] with the thread's ID and posts
///   nothing. When [onRetry] answers that it queued the change, the page
///   reads the thread as working again until the thread changes; when it
///   queued nothing, the failure stays with a note saying so.
/// - **Authors.** Every comment the owner wrote reads "You", whatever name
///   it was posted with: the owner is known by public ID
///   ([ClaxApi.ownerPublicId], read once, and again after the first reply
///   while the owner did not exist). Other viewers keep their name, the
///   agent reads "Clax". Each comment is one screen-reader node, and a
///   failure reply says "failed".
/// - **Resolve** / **Reopen** as the owner.
/// - **Versions.** "Changed in vN" for each version that addressed the
///   thread calls [onPreview].
///
/// The thread is read again on every `thread`, `comment`,
/// `thread_resolved`, `thread_deleted`, `feedback_state` and `version`
/// event of [aid], and on the event stream's `ready` and `resync`.
class ThreadDetail extends StatefulWidget {
  const ThreadDetail({
    super.key,
    required this.api,
    required this.aid,
    required this.tid,
    required this.voice,
    required this.autoDictate,
    required this.onPreview,
    required this.onRetry,
    this.label,
    this.now = DateTime.now,
  });

  final ClaxApi api;
  final String aid;
  final String tid;
  final VoiceInput voice;
  final bool autoDictate;

  /// Shows version `n` of the app.
  final void Function(int n) onPreview;

  /// Runs the thread's failed change again, answering whether it queued
  /// one; null when nothing can, and Retry is not offered.
  final Future<bool> Function(String tid)? onRetry;

  /// What the thread is anchored to in the shell's words; [anchorWords]
  /// of its anchor when null or empty.
  final String? label;

  /// The clock relative times are read against.
  final DateTime Function() now;

  @override
  State<ThreadDetail> createState() => _ThreadDetailState();
}

enum _Action { reply, send, resolve, reopen }

class _ThreadDetailState extends State<ThreadDetail> {
  ThreadView? _thread;
  var _gone = false;
  String? _loadError;
  Uint8List? _clip;
  var _clipAsked = false;

  StreamSubscription<ClaxEvent>? _events;
  var _loading = false;
  var _loadAgain = false;

  final _reply = TextEditingController();
  final _focus = FocusNode();
  _Action? _busy;
  String? _actionError;

  /// The comment count when Retry was tapped; the page reads the thread as
  /// working until the count moves on.
  int? _retriedAt;

  /// The owner's public ID, once read; null before the owner exists.
  String? _owner;

  // Dictation.
  StreamSubscription<VoiceResult>? _listen;
  var _listening = false;
  String? _voiceNote;

  static const _eventTypes = [
    'thread',
    'comment',
    'thread_resolved',
    'thread_deleted',
    'feedback_state',
    'version',
  ];

  @override
  void initState() {
    super.initState();
    _reply.addListener(() => setState(() {}));
    _focus.addListener(_onFocus);
    _events = widget.api
        .events(artifacts: [widget.aid], types: _eventTypes)
        .listen((_) => unawaited(_load()), onError: (Object _) {});
    unawaited(_load());
    unawaited(_readOwner());
  }

  Future<void> _readOwner() async {
    try {
      final id = await widget.api.ownerPublicId();
      if (mounted && id != _owner) setState(() => _owner = id);
    } on ClaxApiError {
      // Owner comments then read by the name they were posted with.
    }
  }

  @override
  void dispose() {
    unawaited(_events?.cancel());
    if (_listen != null) {
      unawaited(_listen!.cancel());
      unawaited(widget.voice.stop());
    }
    _reply.dispose();
    _focus.dispose();
    super.dispose();
  }

  // --- reading ---

  Future<void> _load() async {
    if (_loading) {
      _loadAgain = true;
      return;
    }
    _loading = true;
    try {
      do {
        _loadAgain = false;
        try {
          final all = await widget.api.threads(widget.aid);
          final t = all.where((t) => t.id == widget.tid).firstOrNull;
          if (!mounted) return;
          setState(() {
            _loadError = null;
            _gone = t == null;
            if (t != null) _show(t);
          });
        } on ClaxApiError catch (e) {
          if (!mounted) return;
          setState(() {
            if (e.isNotFound) {
              _gone = true;
            } else {
              _loadError = 'Couldn’t read this thread: ${e.message}';
            }
          });
        }
      } while (_loadAgain);
    } finally {
      _loading = false;
    }
    _fetchClip();
  }

  /// Shows [t]; inside setState.
  void _show(ThreadView t) {
    final retried = _retriedAt;
    if (retried != null && t.comments.length != retried) _retriedAt = null;
    _thread = t;
  }

  void _fetchClip() {
    final t = _thread;
    if (t == null || !t.hasClip || _clipAsked) return;
    _clipAsked = true;
    unawaited(() async {
      try {
        final png = await widget.api.clip(widget.aid, widget.tid);
        if (mounted && png != null) setState(() => _clip = png);
      } on ClaxApiError {
        // The page goes without its clip.
      }
    }());
  }

  // --- acting ---

  Future<void> _act(
    _Action a,
    Future<ThreadView> Function() call,
    String failed,
  ) async {
    if (_busy != null) return;
    setState(() {
      _busy = a;
      _actionError = null;
    });
    try {
      final t = await call();
      if (!mounted) return;
      setState(() {
        _show(t);
        if (a == _Action.reply) _reply.clear();
      });
    } on ClaxApiError catch (e) {
      if (mounted) setState(() => _actionError = '$failed: ${e.message}');
    } finally {
      if (mounted) setState(() => _busy = null);
    }
  }

  Future<void> _send() async {
    final text = _reply.text.trim();
    if (text.isEmpty) return;
    await _stopDictation();
    await _act(
      _Action.reply,
      () => widget.api.postOwnerComment(widget.aid, widget.tid, text),
      'Couldn’t send the reply',
    );
    // The first owner comment makes the owner.
    if (_owner == null) await _readOwner();
  }

  Future<void> _retry() async {
    final t = _thread;
    final retry = widget.onRetry;
    if (t == null || retry == null || _retriedAt != null) return;
    setState(() {
      _retriedAt = t.comments.length;
      _actionError = null;
    });
    final queued = await retry(widget.tid);
    if (!mounted || queued) return;
    setState(() {
      _retriedAt = null;
      _actionError = 'There was nothing to run again. Reply to ask once more.';
    });
  }

  // --- dictation ---

  void _onFocus() {
    if (_focus.hasFocus && widget.autoDictate && !_listening) {
      unawaited(_startDictation());
    }
  }

  Future<void> _toggleMic() =>
      _listening ? _stopDictation() : _startDictation();

  Future<void> _startDictation() async {
    if (_listening) return;
    setState(() {
      _listening = true;
      _voiceNote = null;
    });
    final available = await widget.voice.availability();
    if (!mounted) return;
    switch (available) {
      case VoiceDenied(:final reason) || VoiceUnsupported(:final reason):
        setState(() {
          _listening = false;
          _voiceNote = reason;
        });
        return;
      case VoiceAvailable():
    }
    final before = _reply.text.trim();
    _listen = widget.voice.listen().listen(
      (r) {
        if (!mounted) return;
        final text = before.isEmpty ? r.words : '$before ${r.words}';
        _reply.value = TextEditingValue(
          text: text,
          selection: TextSelection.collapsed(offset: text.length),
        );
      },
      onError: (Object e) {
        if (!mounted) return;
        setState(
          () => _voiceNote = e is VoiceError
              ? 'Dictation stopped: ${e.message}'
              : '$e',
        );
      },
      onDone: () {
        _listen = null;
        if (mounted) setState(() => _listening = false);
      },
    );
  }

  Future<void> _stopDictation() async {
    if (_listen == null) return;
    await widget.voice.stop();
  }

  // --- the view ---

  @override
  Widget build(BuildContext context) {
    final t = _thread;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Thread'),
        actions: [
          if (t != null && t.isOpen)
            TextButton(
              key: const ValueKey('thread-resolve'),
              onPressed: _busy == null
                  ? () => _act(
                      _Action.resolve,
                      () => widget.api.resolveAsOwner(widget.aid, widget.tid),
                      'Couldn’t resolve it',
                    )
                  : null,
              child: const Text('Resolve'),
            )
          else if (t != null)
            TextButton(
              key: const ValueKey('thread-reopen'),
              onPressed: _busy == null
                  ? () => _act(
                      _Action.reopen,
                      () => widget.api.reopenThread(widget.aid, widget.tid),
                      'Couldn’t reopen it',
                    )
                  : null,
              child: const Text('Reopen'),
            ),
          const SizedBox(width: 4),
        ],
      ),
      body: SafeArea(top: false, bottom: false, child: _body(context, t)),
    );
  }

  Widget _body(BuildContext context, ThreadView? t) {
    final theme = Theme.of(context);
    if (_gone) {
      return _message(context, Icons.forum_outlined, 'This thread is gone');
    }
    if (t == null) {
      final error = _loadError;
      if (error == null) {
        return const Center(child: CircularProgressIndicator());
      }
      return _message(
        context,
        Icons.cloud_off_outlined,
        error,
        action: FilledButton.icon(
          onPressed: () => unawaited(_load()),
          icon: const Icon(Icons.refresh),
          label: const Text('Try again'),
        ),
      );
    }
    final label = (widget.label?.isNotEmpty ?? false)
        ? widget.label!
        : anchorWords(t.anchor);
    final now = widget.now();
    return Column(
      children: [
        Expanded(
          child: ListView(
            key: const ValueKey('thread-list'),
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 16),
            children: [
              Text(_sentence(label), style: theme.textTheme.titleLarge),
              const SizedBox(height: 2),
              Text(
                'On version ${t.versionN}',
                style: theme.textTheme.bodySmall?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
              const SizedBox(height: 12),
              _statusCard(context, t),
              if (_clip != null) ...[
                const SizedBox(height: 12),
                _clipView(context, _clip!),
              ],
              if (t.addressedIn.isNotEmpty) ...[
                const SizedBox(height: 12),
                Wrap(
                  spacing: 8,
                  runSpacing: 8,
                  children: [
                    for (final n in t.addressedIn)
                      ActionChip(
                        key: ValueKey('changed-in-$n'),
                        avatar: const Icon(Icons.visibility_outlined, size: 18),
                        label: Text('Changed in v$n'),
                        onPressed: () => widget.onPreview(n),
                      ),
                  ],
                ),
              ],
              const SizedBox(height: 16),
              for (final c in t.comments) _comment(context, c, now),
            ],
          ),
        ),
        _composer(context),
      ],
    );
  }

  Widget _message(
    BuildContext context,
    IconData icon,
    String text, {
    Widget? action,
  }) {
    final theme = Theme.of(context);
    return Center(
      child: SingleChildScrollView(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 40, color: theme.colorScheme.onSurfaceVariant),
            const SizedBox(height: 16),
            Text(
              text,
              textAlign: TextAlign.center,
              style: theme.textTheme.titleMedium,
            ),
            if (action != null) ...[const SizedBox(height: 20), action],
          ],
        ),
      ),
    );
  }

  Widget _statusCard(BuildContext context, ThreadView t) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    var status = threadStatus(t);
    final retrying = _retriedAt != null && status == ThreadStatus.failed;
    if (retrying) status = ThreadStatus.working;
    final failedReason = status == ThreadStatus.failed
        ? _sentence(t.comments.last.body.split(':').skip(1).join(':').trim())
        : '';
    final (
      IconData icon,
      Color bg,
      Color fg,
      String title,
      String line,
    ) = switch (status) {
      ThreadStatus.unsent => (
        Icons.outbox_outlined,
        scheme.surfaceContainerHighest,
        scheme.onSurface,
        'Not sent to Clax',
        'Only you see it. Send it to have Clax make the change.',
      ),
      ThreadStatus.sent => (
        Icons.schedule_send_outlined,
        scheme.secondaryContainer,
        scheme.onSecondaryContainer,
        'Sent to Clax',
        'Waiting for Clax to pick it up.',
      ),
      ThreadStatus.working => (
        Icons.autorenew,
        scheme.secondaryContainer,
        scheme.onSecondaryContainer,
        retrying ? 'Trying again' : 'Clax is working on it',
        'The app stays as it is until the change lands.',
      ),
      ThreadStatus.replied => (
        Icons.forum_outlined,
        scheme.secondaryContainer,
        scheme.onSecondaryContainer,
        'Clax replied',
        'Reply to ask for more, or resolve it.',
      ),
      ThreadStatus.failed => (
        Icons.error_outline,
        scheme.errorContainer,
        scheme.onErrorContainer,
        'Couldn’t make that change',
        failedReason,
      ),
      ThreadStatus.resolved => (
        Icons.check_circle_outline,
        scheme.surfaceContainerHighest,
        scheme.onSurfaceVariant,
        'Resolved',
        'Reopen it to ask for more.',
      ),
    };
    final Widget? action = switch (status) {
      ThreadStatus.unsent => FilledButton.icon(
        key: const ValueKey('send-to-clax'),
        onPressed: _busy == null
            ? () => _act(
                _Action.send,
                () => widget.api.sendThread(widget.aid, widget.tid),
                'Couldn’t send it to Clax',
              )
            : null,
        icon: const Icon(Icons.send, size: 18),
        label: const Text('Send to Clax'),
      ),
      ThreadStatus.failed when widget.onRetry != null => FilledButton.icon(
        key: const ValueKey('thread-retry'),
        style: FilledButton.styleFrom(
          backgroundColor: scheme.error,
          foregroundColor: scheme.onError,
        ),
        onPressed: () => unawaited(_retry()),
        icon: const Icon(Icons.refresh, size: 18),
        label: const Text('Retry'),
      ),
      _ => null,
    };
    return Material(
      key: ValueKey('thread-status-${status.name}'),
      color: bg,
      borderRadius: BorderRadius.circular(16),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(14, 12, 14, 12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Padding(
                  padding: const EdgeInsets.only(top: 2),
                  child: Icon(icon, size: 20, color: fg),
                ),
                const SizedBox(width: 10),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        title,
                        style: theme.textTheme.titleSmall?.copyWith(color: fg),
                      ),
                      if (line.isNotEmpty) ...[
                        const SizedBox(height: 2),
                        Text(
                          line,
                          style: theme.textTheme.bodySmall?.copyWith(color: fg),
                        ),
                      ],
                    ],
                  ),
                ),
              ],
            ),
            if (action != null) ...[
              const SizedBox(height: 10),
              Align(alignment: Alignment.centerRight, child: action),
            ],
          ],
        ),
      ),
    );
  }

  Widget _clipView(BuildContext context, Uint8List png) {
    final scheme = Theme.of(context).colorScheme;
    return Align(
      alignment: Alignment.centerLeft,
      child: Container(
        key: const ValueKey('thread-clip'),
        constraints: const BoxConstraints(maxHeight: 220),
        decoration: BoxDecoration(
          border: Border.all(color: scheme.outlineVariant),
          borderRadius: BorderRadius.circular(12),
        ),
        clipBehavior: Clip.antiAlias,
        child: Image.memory(
          png,
          fit: BoxFit.contain,
          gaplessPlayback: true,
          semanticLabel: 'What the comment was made on',
        ),
      ),
    );
  }

  Widget _comment(BuildContext context, ThreadComment c, DateTime now) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final agent = c.byAgent;
    final owner = _owner;
    final name = agent
        ? 'Clax'
        : (owner != null && c.authorPublicId == owner) || c.authorName.isEmpty
        ? 'You'
        : c.authorName;
    final failed = isAppFailureReply(c);
    final bubble = agent
        ? (failed ? scheme.errorContainer : scheme.secondaryContainer)
        : scheme.surfaceContainerHigh;
    final onBubble = agent
        ? (failed ? scheme.onErrorContainer : scheme.onSecondaryContainer)
        : scheme.onSurface;
    final time = relativeTime(c.createdAt, now);
    return MergeSemantics(
      key: ValueKey('comment-${agent ? 'agent' : 'person'}-${c.id}'),
      child: Padding(
        padding: const EdgeInsets.only(bottom: 14),
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            CircleAvatar(
              radius: 16,
              backgroundColor: agent
                  ? scheme.primary
                  : scheme.tertiaryContainer,
              foregroundColor: agent
                  ? scheme.onPrimary
                  : scheme.onTertiaryContainer,
              child: agent
                  ? const Icon(Icons.auto_awesome, size: 16)
                  : ExcludeSemantics(
                      child: Text(
                        name.characters.first.toUpperCase(),
                        style: const TextStyle(fontWeight: FontWeight.w600),
                      ),
                    ),
            ),
            const SizedBox(width: 10),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Wrap(
                    spacing: 8,
                    crossAxisAlignment: WrapCrossAlignment.end,
                    children: [
                      Text(
                        name,
                        style: theme.textTheme.labelLarge?.copyWith(
                          color: agent ? scheme.primary : null,
                        ),
                      ),
                      if (time.isNotEmpty)
                        Text(
                          time,
                          style: theme.textTheme.labelSmall?.copyWith(
                            color: scheme.onSurfaceVariant,
                          ),
                        ),
                      if (failed)
                        Text(
                          'failed',
                          style: theme.textTheme.labelSmall?.copyWith(
                            color: scheme.error,
                            fontWeight: FontWeight.w700,
                          ),
                        ),
                    ],
                  ),
                  const SizedBox(height: 4),
                  DecoratedBox(
                    decoration: BoxDecoration(
                      color: bubble,
                      borderRadius: const BorderRadius.only(
                        topRight: Radius.circular(14),
                        bottomLeft: Radius.circular(14),
                        bottomRight: Radius.circular(14),
                        topLeft: Radius.circular(4),
                      ),
                    ),
                    child: Padding(
                      padding: const EdgeInsets.fromLTRB(12, 9, 12, 10),
                      child: Text(
                        c.body,
                        style: theme.textTheme.bodyMedium?.copyWith(
                          color: onBubble,
                        ),
                      ),
                    ),
                  ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _composer(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final note = _actionError ?? _voiceNote;
    final noteIsError = _actionError != null || _voiceNote != null;
    final canSend = _reply.text.trim().isNotEmpty && _busy == null;
    return Material(
      color: scheme.surfaceContainerLow,
      child: SafeArea(
        top: false,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(8, 8, 8, 8),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              if (note != null)
                Padding(
                  padding: const EdgeInsets.fromLTRB(8, 0, 8, 6),
                  child: Text(
                    note,
                    key: const ValueKey('reply-note'),
                    style: theme.textTheme.bodySmall?.copyWith(
                      color: noteIsError ? scheme.error : null,
                    ),
                  ),
                ),
              Row(
                crossAxisAlignment: CrossAxisAlignment.end,
                children: [
                  IconButton.filledTonal(
                    key: const ValueKey('reply-mic'),
                    tooltip: _listening ? 'Stop' : 'Dictate',
                    style: _listening
                        ? IconButton.styleFrom(
                            backgroundColor: scheme.primary,
                            foregroundColor: scheme.onPrimary,
                          )
                        : null,
                    onPressed: () => unawaited(_toggleMic()),
                    icon: Icon(_listening ? Icons.graphic_eq : Icons.mic_none),
                  ),
                  const SizedBox(width: 6),
                  Expanded(
                    child: TextField(
                      key: const ValueKey('reply-field'),
                      controller: _reply,
                      focusNode: _focus,
                      minLines: 1,
                      maxLines: 5,
                      textCapitalization: TextCapitalization.sentences,
                      decoration: InputDecoration(
                        hintText: _listening ? 'Listening…' : 'Reply',
                        filled: true,
                        fillColor: scheme.surfaceContainerHighest,
                        isDense: true,
                        contentPadding: const EdgeInsets.symmetric(
                          horizontal: 14,
                          vertical: 12,
                        ),
                        border: OutlineInputBorder(
                          borderRadius: BorderRadius.circular(22),
                          borderSide: BorderSide.none,
                        ),
                      ),
                    ),
                  ),
                  const SizedBox(width: 6),
                  IconButton.filled(
                    key: const ValueKey('reply-send'),
                    tooltip: 'Send',
                    onPressed: canSend ? () => unawaited(_send()) : null,
                    icon: _busy == _Action.reply
                        ? const SizedBox.square(
                            dimension: 18,
                            child: CircularProgressIndicator(strokeWidth: 2),
                          )
                        : const Icon(Icons.send),
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
  }
}
