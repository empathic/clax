import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../agent/failure.dart';
import '../clax/api.dart';
import '../platform/voice.dart';
import 'developer.dart';
import 'kit/kit.dart';
import 'theme.dart';
import 'user_message.dart';

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
/// gave no label for: "Whole app", the quoted text (the anchor's spaced
/// `label` when it has one), "this area" or "this spot". The shell's own label (`ThreadSummary.label`) says more and is
/// preferred.
String anchorWords(Map<String, Object?> anchor) {
  if (anchor['kind'] == 'artifact') return 'Whole app';
  final quote = anchor['label'] ?? anchor['quote'];
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
/// - **Landing.** Once the thread is first read, the page scrolls to
///   [focusComment] (or, with [focusLatestReply], to the newest comment
///   by Clax) and highlights it briefly; without either, or when it is not
///   there, it lands at the newest comment, at the bottom.
/// - **A run's threads.** With [run] of two or more thread IDs (the
///   threads one change answered, this one among them), a bar reads
///   "1 of 3" with Previous and Next, which call [onRun] with that thread.
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
    this.focusComment,
    this.focusLatestReply = false,
    this.run = const [],
    this.onRun,
    this.waitingForConnection,
    this.onCancel,
    this.onTryNow,
    this.isStopped,
  });

  /// The comment to land on and highlight.
  final String? focusComment;

  /// Whether to land on the newest comment by Clax when [focusComment] is
  /// null or not found.
  final bool focusLatestReply;

  /// The thread IDs of the change this thread came from, in order; the
  /// switcher shows for two or more.
  final List<String> run;

  /// Opens thread `tid` of [run] in this page's place.
  final ValueChanged<String>? onRun;

  /// How long a landed-on comment stays highlighted before it fades.
  static const glowFor = Duration(milliseconds: 1400);

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

  /// The threads whose change waits for the Claude API's connection: while
  /// it names this thread and the thread reads as sent or working, the
  /// status says "Waiting for connection" (the change goes on by itself
  /// once the connection is back), with Try now ([onTryNow]) and Cancel
  /// ([onCancel]) when given.
  final ValueListenable<List<String>>? waitingForConnection;

  /// Stops this thread's part of the waiting change; the thread then reads
  /// as stopped ([isStopped]).
  final VoidCallback? onCancel;

  /// Tries the connection at once.
  final VoidCallback? onTryNow;

  /// Whether a thread was stopped (by Cancel, or the change's Stop) and
  /// waits to be sent again: a sent or working one then reads "Cancelled",
  /// with Send again ([onRetry]).
  final bool Function(String tid)? isStopped;

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

  /// The comments whose Details are open.
  final _openDetails = <String>{};

  /// The comment count when Retry was tapped; the page reads the thread as
  /// working until the count moves on.
  int? _retriedAt;

  /// The owner's public ID, once read; null before the owner exists.
  String? _owner;

  // Landing.
  final _scroll = ScrollController();
  var _landed = false;
  String? _highlighted;
  var _glow = false;
  Timer? _fade;

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
    _scroll.dispose();
    _fade?.cancel();
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
          if (!e.isNotFound) logError('thread', e);
          setState(() {
            if (e.isNotFound) {
              _gone = true;
            } else {
              _loadError = 'Couldn’t read this thread. Try again.';
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
    // The first read lands at the newest comment; the focused one is lit
    // on the first read that holds it.
    final first = !_landed;
    _landed = true;
    final target = _wanted && _highlighted == null ? _target(t) : null;
    if (target != null) {
      _highlighted = target;
      _glow = true;
    }
    if (first || target != null) {
      WidgetsBinding.instance.addPostFrameCallback((_) => _land(target));
    }
  }

  /// Whether the page was asked to land on a comment.
  bool get _wanted => widget.focusComment != null || widget.focusLatestReply;

  /// The comment to land on in [t], if it holds it yet: [focusComment],
  /// else (with [focusLatestReply]) Clax's reply after the person's last
  /// comment, not an earlier one while the reply is not there yet.
  String? _target(ThreadView t) {
    final ids = {for (final c in t.comments) c.id};
    if (widget.focusComment case final id? when ids.contains(id)) return id;
    if (!widget.focusLatestReply) return null;
    final last = t.comments.lastOrNull;
    return last != null && last.byAgent ? last.id : null;
  }

  /// Scrolls to the newest comment, then to [target] and lights it.
  void _land(String? target) {
    if (!mounted) return;
    _toBottom();
    if (target == null) return;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      final ctx = _commentKeys[target]?.currentContext;
      if (ctx != null) {
        unawaited(
          Scrollable.ensureVisible(
            ctx,
            alignment: 0.2,
            duration: const Duration(milliseconds: 250),
            curve: Curves.easeOutCubic,
          ),
        );
      }
      _fade?.cancel();
      _fade = Timer(ThreadDetail.glowFor, () {
        if (mounted) setState(() => _glow = false);
      });
    });
  }

  void _toBottom() {
    if (_scroll.hasClients) _scroll.jumpTo(_scroll.position.maxScrollExtent);
  }

  bool get _atBottom =>
      _scroll.hasClients &&
      _scroll.offset >= _scroll.position.maxScrollExtent - 1;

  final _commentKeys = <String, GlobalKey>{};

  void _fetchClip() {
    final t = _thread;
    if (t == null || !t.hasClip || _clipAsked) return;
    _clipAsked = true;
    unawaited(() async {
      try {
        final png = await widget.api.clip(widget.aid, widget.tid);
        if (!mounted || png == null) return;
        // The clip goes above the comments: a page at the newest comment
        // stays there.
        final bottom = _atBottom && _highlighted == null;
        setState(() => _clip = png);
        if (bottom) {
          WidgetsBinding.instance.addPostFrameCallback((_) => _toBottom());
        }
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
      logError('thread', e);
      if (mounted) setState(() => _actionError = '$failed. Try again.');
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
        if (e is VoiceError) logError('dictation', e);
        setState(
          () => _voiceNote = e is VoiceError
              ? 'Dictation stopped. Try again.'
              : 'Dictation stopped. ${logUserMessage('dictation', e)}',
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
    final run = widget.run;
    final at = run.indexOf(widget.tid);
    return Scaffold(
      appBar: AppBar(
        title: const Text('Thread'),
        bottom: run.length > 1 && at >= 0 && widget.onRun != null
            ? _runBar(context, run, at)
            : null,
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

  PreferredSizeWidget _runBar(BuildContext context, List<String> run, int at) {
    final theme = Theme.of(context);
    return PreferredSize(
      preferredSize: const Size.fromHeight(44),
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 0, 8, 4),
        child: Row(
          children: [
            Expanded(
              child: Text(
                '${at + 1} of ${run.length} threads in this change',
                key: const ValueKey('thread-run-place'),
                style: theme.textTheme.labelLarge?.copyWith(
                  color: theme.colorScheme.onSurfaceVariant,
                ),
              ),
            ),
            IconButton(
              key: const ValueKey('thread-run-previous'),
              tooltip: 'Previous thread',
              onPressed: at > 0 ? () => widget.onRun!(run[at - 1]) : null,
              icon: const Icon(Icons.chevron_left),
            ),
            TextButton.icon(
              key: const ValueKey('thread-run-next'),
              onPressed: at < run.length - 1
                  ? () => widget.onRun!(run[at + 1])
                  : null,
              iconAlignment: IconAlignment.end,
              icon: const Icon(Icons.chevron_right),
              label: const Text('Next'),
            ),
          ],
        ),
      ),
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
          // Laid out whole (a thread is short), so landing knows where its
          // end and each comment are.
          child: SingleChildScrollView(
            key: const ValueKey('thread-list'),
            controller: _scroll,
            padding: const EdgeInsets.fromLTRB(16, 8, 16, 16),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
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
                if (widget.waitingForConnection case final w?)
                  ValueListenableBuilder(
                    valueListenable: w,
                    builder: (context, waiting, _) =>
                        _statusCard(
                          context,
                          t,
                          offline: waiting.contains(widget.tid),
                        ) ??
                        const SizedBox.shrink(),
                  )
                else ?_statusCard(context, t),
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
                          avatar: const Icon(
                            Icons.visibility_outlined,
                            size: 18,
                          ),
                          label: Text('Changed in v$n'),
                          onPressed: () => widget.onPreview(n),
                        ),
                    ],
                  ),
                ],
                const SizedBox(height: 16),
                for (final c in t.comments) _comment(context, t, c, now),
              ],
            ),
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

  /// Whether [t]'s failed change offers Keep going: its last comment is a
  /// limit stop's reply with the Keep going line ([isKeepGoingReply]).
  bool _keepGoing(ThreadView t) =>
      widget.onRetry != null &&
      t.comments.isNotEmpty &&
      isKeepGoingReply(t.comments.last);

  /// The card above the conversation saying where [t] stands; null for a
  /// failed change, which its reply shows ([_comment]).
  Widget? _statusCard(
    BuildContext context,
    ThreadView t, {
    bool offline = false,
  }) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    var status = threadStatus(t);
    final pending =
        status == ThreadStatus.working || status == ThreadStatus.sent;
    final stopped = pending && (widget.isStopped?.call(widget.tid) ?? false);
    final waiting = offline && pending && !stopped;
    // A change that stopped at a limit offers Keep going, whether it
    // published part of itself (replied) or nothing (failed).
    final keepGoing =
        _keepGoing(t) &&
        (status == ThreadStatus.failed || status == ThreadStatus.replied);
    final retrying =
        _retriedAt != null && (status == ThreadStatus.failed || keepGoing);
    if (retrying) status = ThreadStatus.working;
    final look = switch (status) {
      _ when stopped => (
        Icons.cancel_outlined,
        scheme.surfaceContainerHighest,
        scheme.onSurface,
        'Cancelled',
        'Clax will not make this change. Send it again to have it made.',
      ),
      _ when waiting => (
        Icons.cloud_off_outlined,
        scheme.secondaryContainer,
        scheme.onSecondaryContainer,
        'Waiting for connection',
        'It goes to Clax by itself when the connection is back.',
      ),
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
        retrying
            ? (keepGoing ? 'Keeping going' : 'Trying again')
            : 'Clax is working on it',
        'The app stays as it is until the change lands.',
      ),
      ThreadStatus.replied => (
        Icons.forum_outlined,
        scheme.secondaryContainer,
        scheme.onSecondaryContainer,
        'Clax replied',
        keepGoing
            ? 'Only part of the change landed: Keep going builds the rest.'
            : 'Reply to ask for more, or resolve it.',
      ),
      ThreadStatus.failed => null,
      ThreadStatus.resolved => (
        Icons.check_circle_outline,
        scheme.surfaceContainerHighest,
        scheme.onSurfaceVariant,
        'Resolved',
        'Reopen it to ask for more.',
      ),
    };
    if (look == null) return null;
    final (icon, bg, fg, title, line) = look;
    final Widget? action = switch (status) {
      _ when stopped =>
        widget.onRetry == null
            ? null
            : FilledButton.icon(
                key: const ValueKey('thread-send-again'),
                onPressed: () => unawaited(_retry()),
                icon: const Icon(Icons.send, size: 18),
                label: const Text('Send again'),
              ),
      _ when waiting && (widget.onCancel != null || widget.onTryNow != null) =>
        Wrap(
          spacing: 8,
          children: [
            if (widget.onTryNow case final tryNow?)
              TextButton(
                key: const ValueKey('thread-try-now'),
                onPressed: tryNow,
                child: const Text('Try now'),
              ),
            if (widget.onCancel case final cancel?)
              OutlinedButton(
                key: const ValueKey('thread-cancel'),
                onPressed: () {
                  cancel();
                  setState(() {});
                },
                child: const Text('Cancel'),
              ),
          ],
        ),
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
      ThreadStatus.replied when keepGoing => FilledButton.icon(
        key: const ValueKey('thread-keep-going'),
        onPressed: () => unawaited(_retry()),
        icon: const Icon(Icons.play_arrow, size: 18),
        label: const Text('Keep going'),
      ),
      _ => null,
    };
    return Material(
      key: ValueKey(
        'thread-status-${stopped
            ? 'stopped'
            : waiting
            ? 'offline'
            : status.name}',
      ),
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

  Widget _comment(
    BuildContext context,
    ThreadView t,
    ThreadComment c,
    DateTime now,
  ) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final p = claxPalette(context);
    final agent = c.byAgent;
    final owner = _owner;
    final name = agent
        ? 'Clax'
        : (owner != null && c.authorPublicId == owner) || c.authorName.isEmpty
        ? 'You'
        : c.authorName;
    final failed = isAppFailureReply(c);
    // A failure, or a limit stop's progress, reads as one line, with the
    // rest under Details.
    final reply =
        failed ||
            (agent &&
                c.viaHarness == appHarness &&
                parseLimitProgress(c.body) != null)
        ? parseFailureReply(c.body)
        : null;
    final raw = reply != null && DeveloperScope.of(context) ? reply.raw : null;
    final hasDetails = reply != null && (reply.hasDetails || raw != null);
    final open = hasDetails && _openDetails.contains(c.id);
    final verbs = ClaxVerbs.of(context);
    final text = agent ? keepGoingIn(reply?.line ?? c.body, verbs) : c.body;
    final bubble = failed
        ? p.card
        : agent
        ? scheme.secondaryContainer
        : scheme.surfaceContainerHigh;
    final onBubble = failed
        ? p.fg
        : agent
        ? scheme.onSecondaryContainer
        : scheme.onSurface;
    final time = relativeTime(c.createdAt, now);
    final lit = _glow && _highlighted == c.id;

    // Retry or Keep going, on the failure that ends the thread.
    final acts =
        failed &&
        t.comments.last.id == c.id &&
        threadStatus(t) == ThreadStatus.failed &&
        widget.onRetry != null &&
        _retriedAt == null;
    final Widget? action = !acts
        ? null
        : _keepGoing(t)
        ? ClaxButton(
            key: const ValueKey('thread-keep-going'),
            label: 'Keep going',
            icon: Icons.play_arrow,
            kind: ClaxButtonKind.primary,
            isDefault: true,
            onPressed: () => unawaited(_retry()),
          )
        : reply!.spentLimit
        ? null
        : ClaxButton(
            key: const ValueKey('thread-retry'),
            label: 'Retry',
            icon: Icons.refresh,
            kind: ClaxButtonKind.tonal,
            onPressed: () => unawaited(_retry()),
          );
    final Widget? toggle = !hasDetails
        ? null
        : Semantics(
            expanded: open,
            child: ClaxButton(
              key: const ValueKey('failure-details'),
              label: 'Details',
              icon: open ? Icons.expand_less : Icons.expand_more,
              onPressed: () => setState(
                () => open
                    ? _openDetails.remove(c.id)
                    : _openDetails.add(c.id),
              ),
            ),
          );

    final body = Text(
      text,
      style: theme.textTheme.bodyMedium?.copyWith(color: onBubble),
    );
    final content = Padding(
      padding: const EdgeInsets.fromLTRB(12, 9, 12, 10),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ExcludeSemantics(
            child: failed
                ? Row(
                    key: const ValueKey('failure-reply'),
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Padding(
                        padding: const EdgeInsets.only(top: 1),
                        child: Icon(
                          Icons.error_outline,
                          size: 18,
                          color: p.danger,
                        ),
                      ),
                      const SizedBox(width: 8),
                      Expanded(child: body),
                    ],
                  )
                : body,
          ),
          if (action != null || toggle != null) ...[
            const SizedBox(height: 6),
            Wrap(
              spacing: 4,
              runSpacing: 4,
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [?action, ?toggle],
            ),
          ],
          if (open)
            _details(context, reply, raw, onBubble),
        ],
      ),
    );
    const shape = BorderRadius.only(
      topRight: Radius.circular(14),
      bottomLeft: Radius.circular(14),
      bottomRight: Radius.circular(14),
      topLeft: Radius.circular(4),
    );
    final Widget box = failed
        // A neutral card with a danger rule down its left edge.
        ? DecoratedBox(
            decoration: BoxDecoration(
              color: bubble,
              borderRadius: shape,
              border: Border.all(color: p.border),
            ),
            child: ClipRRect(
              borderRadius: shape,
              child: IntrinsicHeight(
                child: Row(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    SizedBox(width: 3, child: ColoredBox(color: p.danger)),
                    Expanded(child: content),
                  ],
                ),
              ),
            ),
          )
        : DecoratedBox(
            decoration: BoxDecoration(color: bubble, borderRadius: shape),
            child: content,
          );
    return Semantics(
      key: ValueKey('comment-${agent ? 'agent' : 'person'}-${c.id}'),
      container: true,
      label: [
        name,
        if (time.isNotEmpty) time,
        if (failed) 'failed',
        text,
      ].join(', '),
      child: AnimatedContainer(
        key: _commentKeys.putIfAbsent(c.id, GlobalKey.new),
        duration: const Duration(milliseconds: 700),
        curve: Curves.easeOut,
        margin: const EdgeInsets.only(bottom: 2),
        padding: const EdgeInsets.all(6),
        decoration: BoxDecoration(
          color: lit
              ? scheme.primaryContainer.withValues(alpha: 0.7)
              : scheme.primaryContainer.withValues(alpha: 0),
          borderRadius: BorderRadius.circular(16),
        ),
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
                  ExcludeSemantics(
                    child: Wrap(
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
                              color: p.danger,
                              fontWeight: FontWeight.w700,
                            ),
                          ),
                      ],
                    ),
                  ),
                  const SizedBox(height: 4),
                  box,
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  /// What a failure or a limit stop's reply keeps under Details: the
  /// stops and the cost ("Stopped twice · $2.10 of $6 used"), the files
  /// changed with their lines, the notes as Markdown, and last [raw].
  Widget _details(
    BuildContext context,
    FailureReply r,
    String? raw,
    Color fg,
  ) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final small = theme.textTheme.bodySmall?.copyWith(color: fg);
    final muted = theme.textTheme.bodySmall?.copyWith(color: p.muted);
    Widget heading(String s) => Padding(
      padding: const EdgeInsets.only(top: 10, bottom: 4),
      child: Text(
        s,
        style: theme.textTheme.labelMedium?.copyWith(color: p.muted),
      ),
    );
    String usd(double v) =>
        v == v.roundToDouble() ? v.toStringAsFixed(0) : v.toStringAsFixed(2);
    final cost = r.costUsd == null || r.capUsd == null
        ? null
        : '${r.stops == null ? '' : 'Stopped ${timesWords(r.stops!)} · '}'
              '\$${r.costUsd!.toStringAsFixed(2)} of \$${usd(r.capUsd!)} used';
    return Padding(
      key: const ValueKey('failure-details-open'),
      padding: const EdgeInsets.only(top: 2),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          if (cost != null) ...[const SizedBox(height: 6), Text(cost, style: small)],
          if (r.files.isNotEmpty) ...[
            heading('Changed'),
            for (final f in r.files)
              Padding(
                padding: const EdgeInsets.symmetric(vertical: 1),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        f.path,
                        style: small?.copyWith(fontFamily: 'monospace'),
                        overflow: TextOverflow.ellipsis,
                      ),
                    ),
                    if (f.added > 0 || f.removed > 0) ...[
                      const SizedBox(width: 8),
                      Text(
                        [
                          if (f.added > 0) '+${f.added}',
                          if (f.removed > 0) '−${f.removed}',
                        ].join(' '),
                        style: muted?.copyWith(
                          fontFeatures: const [FontFeature.tabularFigures()],
                        ),
                      ),
                    ],
                  ],
                ),
              ),
            if (r.moreFiles > 0) Text('and ${r.moreFiles} more', style: muted),
          ],
          if (r.notes.isNotEmpty) ...[
            heading('Notes'),
            for (final n in r.notes) ..._markdown(n, small, bullet: true),
          ],
          if (raw != null) ...[
            heading('Error'),
            SelectableText(
              raw,
              style: small?.copyWith(fontFamily: 'monospace'),
            ),
          ],
        ],
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

/// [md] as widgets: each line opening "- " or "* " a bullet, other lines
/// paragraphs, `**bold**` and `` `code` `` inline. With [bullet], a
/// paragraph line is a bullet too (one note of a list).
List<Widget> _markdown(String md, TextStyle? style, {bool bullet = false}) {
  Widget item(String s, {required bool dot}) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 1),
    child: dot
        ? Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              SizedBox(width: 14, child: Text('•', style: style)),
              Expanded(child: Text.rich(_inline(s, style))),
            ],
          )
        : Text.rich(_inline(s, style)),
  );
  final lines = md
      .split('\n')
      .map((l) => l.trimRight())
      .where((l) => l.trim().isNotEmpty)
      .toList();
  final listed = lines.any((l) => RegExp(r'^\s*[-*] ').hasMatch(l));
  return [
    for (final l in lines)
      switch (RegExp(r'^\s*[-*] (.*)$').firstMatch(l)) {
        final m? => item(m[1]!, dot: true),
        // A note that is its own list keeps its lead line plain.
        _ => item(l.trim(), dot: bullet && !listed),
      },
  ];
}

/// [s]'s `**bold**` and `` `code` `` spans in [style].
TextSpan _inline(String s, TextStyle? style) {
  final spans = <TextSpan>[];
  var at = 0;
  for (final m in RegExp(r'\*\*(.+?)\*\*|`([^`]+)`').allMatches(s)) {
    if (m.start > at) spans.add(TextSpan(text: s.substring(at, m.start)));
    spans.add(
      m[1] != null
          ? TextSpan(
              text: m[1],
              style: const TextStyle(fontWeight: FontWeight.w600),
            )
          : TextSpan(
              text: m[2],
              style: const TextStyle(fontFamily: 'monospace'),
            ),
    );
    at = m.end;
  }
  if (at < s.length) spans.add(TextSpan(text: s.substring(at)));
  return TextSpan(style: style, children: spans);
}
