import 'dart:async';
import 'dart:math';

import 'package:flutter/material.dart';

import '../clax/api.dart';
import 'haiku.dart';
import 'kit/kit.dart';
import 'launcher.dart' show tileScheme;
import 'user_message.dart';

/// The owner's inbox across apps: the agents' replies, new versions, new
/// apps, questions and finished work, newest first, live.
///
/// On open it subscribes [stream] (the stream it is given, which it never
/// closes, else one of its own) to `inbox` and `questions` (so the app
/// counts as an owner surface while the inbox is open), then fetches
/// the first page, then applies `inbox_item` events on top: each one
/// upserts its item by ID. `inbox_read` and `resync` refetch. Events that
/// arrive while a fetch is in flight are applied again on its result.
///
/// **Unread / All**, the kind chips and the search field (after a pause in
/// typing of [searchDelay]) change the query; scrolling to the end fetches
/// the next page with `before`. A new item that arrives live is added when
/// it matches the read filter and the kind chips; while a search is
/// active only items already shown are updated, since the daemon decides
/// what a search matches.
///
/// A tap marks the item read and passes it to [onOpen]; an item whose
/// source is gone is only marked read. **Mark all read** marks every unread
/// item matching the current filter up to the newest one shown.
///
/// With nothing left to read it says "All caught up." over a haiku, a new
/// one each visit; a long-press (Option-click on the Mac) shows another.
class InboxScreen extends StatefulWidget {
  const InboxScreen({
    super.key,
    required this.api,
    required this.onOpen,
    this.timer,
    this.now,
    this.searchDelay = const Duration(milliseconds: 300),
    this.stream,
    this.retryDelay,
    this.haikus,
    this.random,
  });

  final ClaxApi api;

  /// The haikus "All caught up." picks from; null reads the app's list
  /// ([loadHaikus]).
  final Future<List<String>> Function()? haikus;

  /// How the haiku is picked; null is a fresh [Random].
  final Random? random;

  /// Opens what [InboxItem] points at: its thread, version, app or
  /// question.
  final void Function(InboxItem item) onOpen;

  /// Starts the search's pause timer; [Timer.new] when null.
  final Timer Function(Duration, void Function())? timer;

  /// The clock the rows' times are relative to; [DateTime.now] when null.
  final DateTime Function()? now;

  final Duration searchDelay;

  /// The stream to follow the inbox on, which the screen leaves open; when
  /// null the screen opens its own and closes it as it goes.
  final ClaxStream? stream;

  /// Waits between tries to subscribe a topic again; [Future.delayed]
  /// when null.
  final Future<void> Function(Duration)? retryDelay;

  @override
  State<InboxScreen> createState() => _InboxScreenState();
}

/// The kind chips: each item kind and its chip's label.
const inboxKinds = [
  ('reply', 'Replies'),
  ('version', 'Versions'),
  ('published', 'New apps'),
  ('question', 'Questions'),
  ('finished', 'Finished'),
];

class _InboxScreenState extends State<InboxScreen> {
  ClaxStream? _stream;
  StreamSubscription<StreamEvent>? _events;

  List<InboxItem>? _items;
  String? _next;
  int _unread = 0;
  Object? _loadError;
  bool _pageError = false;
  bool _paging = false;

  bool _unreadOnly = false;
  final List<String> _kinds = [];
  final _search = TextEditingController();
  String _query = '';
  Timer? _searchTimer;

  /// Bumped by each first-page fetch, so only the newest one's answer is
  /// kept and a page fetched for an older query is dropped.
  int _loads = 0;

  /// The `inbox_item` events received while a first-page fetch was in
  /// flight, applied again on its result.
  List<InboxItem>? _duringLoad;

  /// The topics being subscribed again after an `unsubscribed`.
  final Set<String> _resubscribing = {};

  List<String> _haikus = const [];
  String? _haiku;

  /// The haiku the inbox showed last, which the next visit does not
  /// repeat.
  static String? _lastHaiku;

  @override
  void initState() {
    super.initState();
    unawaited(_start());
    unawaited(_loadHaikus());
  }

  /// Reads the haikus apart from the list, so a slow one never holds the
  /// inbox back.
  Future<void> _loadHaikus() async {
    // After initState, where the asset bundle can be looked up.
    await Future<void>.value();
    if (!mounted) return;
    final bundle = DefaultAssetBundle.of(context);
    final List<String> haikus;
    try {
      haikus = await (widget.haikus?.call() ?? loadHaikus(bundle));
    } on Object {
      return;
    }
    if (!mounted) return;
    setState(() {
      _haikus = haikus;
      _lastHaiku = _haiku = _pick(_lastHaiku);
    });
  }

  /// A haiku other than [not] where there is another; null with none.
  String? _pick(String? not) {
    if (_haikus.isEmpty) return null;
    final others = [for (final h in _haikus) if (h != not) h];
    return pickHaiku(others.isEmpty ? _haikus : others, random: widget.random);
  }

  void _anotherHaiku() => setState(() => _lastHaiku = _haiku = _pick(_haiku));

  Future<void> _start() async {
    final stream = _stream = widget.stream ?? widget.api.stream();
    _events = stream.events.listen(_onEvent, onError: (Object _) {});
    try {
      await stream.subscribe(const ['inbox', 'questions']);
    } catch (_) {
      // The list still loads; it only misses live changes until a refresh.
    }
    if (mounted) await _refresh();
  }

  @override
  void dispose() {
    _searchTimer?.cancel();
    _search.dispose();
    unawaited(_events?.cancel());
    if (widget.stream == null) unawaited(_stream?.close());
    super.dispose();
  }

  String get _read => _unreadOnly ? 'unread' : 'all';

  Future<void> _refresh() async {
    final load = ++_loads;
    _duringLoad = [];
    try {
      final page = await widget.api.inbox(
        q: _query.isEmpty ? null : _query,
        kinds: _kinds.isEmpty ? null : List.of(_kinds),
        read: _read,
      );
      if (!mounted || load != _loads) return;
      final late = _duringLoad ?? const [];
      _duringLoad = null;
      setState(() {
        _items = page.items;
        _next = page.nextCursor;
        _unread = page.unread;
        _loadError = null;
        _pageError = false;
        late.forEach(_upsert);
      });
    } catch (e) {
      if (!mounted || load != _loads) return;
      _duringLoad = null;
      setState(() => _loadError = e);
    }
  }

  Future<void> _loadMore() async {
    final next = _next;
    if (next == null || _paging || _pageError) return;
    _paging = true;
    final load = _loads;
    try {
      final page = await widget.api.inbox(
        q: _query.isEmpty ? null : _query,
        kinds: _kinds.isEmpty ? null : List.of(_kinds),
        read: _read,
        before: next,
      );
      if (!mounted || load != _loads) return;
      setState(() {
        final shown = {for (final i in _items!) i.id};
        _items = [
          ..._items!,
          ...page.items.where((i) => !shown.contains(i.id)),
        ];
        _next = page.nextCursor;
        _unread = page.unread;
      });
    } catch (_) {
      if (mounted && load == _loads) setState(() => _pageError = true);
    } finally {
      _paging = false;
    }
  }

  void _onEvent(StreamEvent e) {
    if (!mounted) return;
    switch (e.type) {
      case 'unsubscribed':
        if (e.data['topic'] case final String topic) {
          unawaited(_resubscribe(topic));
        }
      case 'resync' when e.data['topic'] == 'inbox':
        unawaited(_refresh());
      case 'inbox_read':
        unawaited(_refresh());
      case 'inbox_item':
        final InboxItem item;
        try {
          item = InboxItem.fromJson(e.data['item']! as Map<String, Object?>);
        } catch (_) {
          return;
        }
        _duringLoad?.add(item);
        setState(() {
          if (e.data['unread'] case final num n) _unread = n.toInt();
          _upsert(item);
        });
    }
  }

  /// Subscribes [topic] again after the stream dropped it, then refetches
  /// what an `inbox` gap may have missed.
  Future<void> _resubscribe(String topic) async {
    final stream = _stream;
    if (stream == null || !_resubscribing.add(topic)) return;
    try {
      final ok = await resubscribe(
        stream,
        [topic],
        delay: widget.retryDelay ?? Future.delayed,
        alive: () => mounted,
      );
      if (ok && mounted && topic == 'inbox') await _refresh();
    } finally {
      _resubscribing.remove(topic);
    }
  }

  /// Replaces the shown item with [item]'s ID, or adds [item] in `seq`
  /// order when it matches the filter and falls within the pages shown.
  void _upsert(InboxItem item) {
    final items = _items;
    if (items == null) return;
    final at = items.indexWhere((i) => i.id == item.id);
    if (at >= 0) {
      _items = List.of(items)..[at] = item;
      return;
    }
    if (_query.isNotEmpty) return;
    if (_unreadOnly && item.read) return;
    if (_kinds.isNotEmpty && !_kinds.contains(item.kind)) return;
    if (_next != null && items.isNotEmpty && item.seq < items.last.seq) {
      return;
    }
    _items = [...items, item]..sort((a, b) => b.seq.compareTo(a.seq));
  }

  void _setFilter(void Function() change) {
    setState(() {
      change();
      _items = null;
      _loadError = null;
    });
    unawaited(_refresh());
  }

  void _onSearch(String text) {
    _searchTimer?.cancel();
    _searchTimer = (widget.timer ?? Timer.new)(widget.searchDelay, () {
      final q = text.trim();
      if (q == _query) return;
      _setFilter(() => _query = q);
    });
  }

  void _clearSearch() {
    _search.clear();
    _searchTimer?.cancel();
    if (_query.isNotEmpty) _setFilter(() => _query = '');
  }

  void _markRead(InboxItem item) {
    if (item.read) return;
    setState(() => _upsert(_asRead(item)));
    widget.api.markInboxRead(item.id).then((unread) {
      if (mounted) setState(() => _unread = unread);
    }, onError: (Object _) {});
  }

  void _tap(InboxItem item) {
    _markRead(item);
    if (item.gone) {
      ScaffoldMessenger.of(context)
        ..hideCurrentSnackBar()
        ..showSnackBar(
          const SnackBar(content: Text('This is no longer available')),
        );
      return;
    }
    widget.onOpen(item);
  }

  Future<void> _markAllRead() async {
    final items = _items;
    if (items == null || items.isEmpty) return;
    final upto = items.map((i) => i.seq).reduce((a, b) => a > b ? a : b);
    final filter = <String, Object?>{
      if (_query.isNotEmpty) 'q': _query,
      if (_kinds.isNotEmpty) 'kind': List.of(_kinds),
    };
    setState(() {
      _items = [for (final i in items) i.seq <= upto ? _asRead(i) : i];
    });
    try {
      final unread = await widget.api.markAllInboxRead(
        upto: upto,
        filter: filter.isEmpty ? null : filter,
      );
      if (mounted) setState(() => _unread = unread);
    } catch (_) {
      if (!mounted) return;
      ScaffoldMessenger.of(
        context,
      ).showSnackBar(const SnackBar(content: Text('Couldn’t mark them read')));
      unawaited(_refresh());
    }
  }

  @override
  Widget build(BuildContext context) {
    final items = _items;
    final now = (widget.now ?? DateTime.now)();
    final anyUnread = items?.any((i) => !i.read) ?? false;
    return Scaffold(
      body: RefreshIndicator(
        onRefresh: _refresh,
        edgeOffset: 152,
        child: CustomScrollView(
          physics: const AlwaysScrollableScrollPhysics(),
          slivers: [
            SliverAppBar.large(
              title: Text(_unread > 0 ? 'Inbox · $_unread' : 'Inbox'),
              actions: [
                IconButton(
                  tooltip: 'Mark all read',
                  onPressed: anyUnread ? _markAllRead : null,
                  icon: const Icon(Icons.done_all_rounded),
                ),
                const SizedBox(width: 4),
              ],
            ),
            SliverToBoxAdapter(child: _filters(context)),
            if ((items, _loadError) case (null, final e?))
              SliverFillRemaining(
                hasScrollBody: true,
                child: ClaxEmpty(
                  key: const ValueKey('inbox-error'),
                  icon: Icons.inbox_outlined,
                  title: 'Couldn’t show your inbox',
                  body: logUserMessage('inbox', e),
                  actions: [
                    ClaxButton(
                      label: 'Try again',
                      kind: ClaxButtonKind.primary,
                      icon: Icons.refresh_rounded,
                      onPressed: () => unawaited(_refresh()),
                    ),
                  ],
                ),
              )
            else if (items == null)
              const SliverFillRemaining(
                hasScrollBody: false,
                child: Center(child: CircularProgressIndicator()),
              )
            else if (items.isEmpty)
              SliverFillRemaining(hasScrollBody: true, child: _empty())
            else
              SliverPadding(
                padding: const EdgeInsets.only(bottom: 24),
                sliver: SliverList.builder(
                  itemCount: items.length + (_next != null ? 1 : 0),
                  itemBuilder: (context, i) {
                    if (i == items.length) {
                      if (!_pageError) unawaited(_loadMore());
                      return _PageEnd(
                        failed: _pageError,
                        onRetry: () {
                          setState(() => _pageError = false);
                          unawaited(_loadMore());
                        },
                      );
                    }
                    final item = items[i];
                    return InboxRow(
                      key: ValueKey(item.id),
                      item: item,
                      now: now,
                      onTap: () => _tap(item),
                    );
                  },
                ),
              ),
          ],
        ),
      ),
    );
  }

  Widget _filters(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
          child: TextField(
            controller: _search,
            onChanged: _onSearch,
            onSubmitted: (_) {
              _searchTimer?.cancel();
              _commitSearch();
            },
            textInputAction: TextInputAction.search,
            decoration: InputDecoration(
              hintText: 'Search the inbox',
              prefixIcon: const Icon(Icons.search_rounded),
              suffixIcon: ListenableBuilder(
                listenable: _search,
                builder: (context, _) => _search.text.isEmpty
                    ? const SizedBox.shrink()
                    : IconButton(
                        tooltip: 'Clear search',
                        onPressed: _clearSearch,
                        icon: const Icon(Icons.close_rounded),
                      ),
              ),
              border: OutlineInputBorder(
                borderRadius: BorderRadius.circular(28),
                borderSide: BorderSide.none,
              ),
              contentPadding: const EdgeInsets.symmetric(vertical: 14),
            ),
          ),
        ),
        SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          padding: const EdgeInsets.fromLTRB(16, 0, 16, 8),
          child: Row(
            children: [
              FilterChip(
                label: const Text('Unread'),
                selected: _unreadOnly,
                onSelected: (on) => _setFilter(() => _unreadOnly = on),
              ),
              Container(
                width: 1,
                height: 24,
                margin: const EdgeInsets.symmetric(horizontal: 8),
                color: Theme.of(context).colorScheme.outlineVariant,
              ),
              for (final (kind, label) in inboxKinds) ...[
                FilterChip(
                  label: Text(label),
                  selected: _kinds.contains(kind),
                  onSelected: (on) => _setFilter(() {
                    on ? _kinds.add(kind) : _kinds.remove(kind);
                  }),
                ),
                const SizedBox(width: 8),
              ],
            ],
          ),
        ),
      ],
    );
  }

  void _commitSearch() {
    final q = _search.text.trim();
    if (q != _query) _setFilter(() => _query = q);
  }

  Widget _empty() {
    if (_query.isNotEmpty || _kinds.isNotEmpty) {
      return const ClaxEmpty(
        key: ValueKey('inbox-no-matches'),
        icon: Icons.search_off_rounded,
        title: 'No matches',
        body: 'Try other words, or fewer filters.',
      );
    }
    final haiku = _haiku;
    return ClaxEmpty(
      key: const ValueKey('inbox-caught-up'),
      icon: Icons.mark_email_read_outlined,
      title: 'All caught up.',
      // Unread only: there may be more under All.
      body: _unreadOnly
          ? null
          : 'Replies, new versions and Clax’s questions show up here.',
      extra: haiku == null
          ? null
          : HaikuLine(
              key: const ValueKey('inbox-haiku'),
              text: haiku,
              onAnother: _haikus.length > 1 ? _anotherHaiku : null,
            ),
    );
  }
}

/// [item] marked read.
InboxItem _asRead(InboxItem item) => InboxItem(
  id: item.id,
  seq: item.seq,
  kind: item.kind,
  read: true,
  createdAt: item.createdAt,
  url: item.url,
  gone: item.gone,
  artifactId: item.artifactId,
  artifactTitle: item.artifactTitle,
  artifactKind: item.artifactKind,
  agentHandle: item.agentHandle,
  agentHarness: item.agentHarness,
  agentProject: item.agentProject,
  thread: item.thread,
  replyCommentId: item.replyCommentId,
  replyBody: item.replyBody,
  versionN: item.versionN,
  versionNote: item.versionNote,
  versionAddressed: item.versionAddressed,
  publishedDescription: item.publishedDescription,
  question: item.question,
  workMessage: item.workMessage,
  workThreads: item.workThreads,
);

/// The line under an item's title: the reply's text, `v5: <note>`, "New
/// app", the question's header, or `Finished: <message>`.
String inboxLine(InboxItem item) {
  if (item.gone) return 'No longer available';
  switch (item.kind) {
    case 'reply':
      final body = plainPreview(item.replyBody ?? '');
      return body.isEmpty ? 'Replied' : body;
    case 'version':
      final n = item.versionN;
      final note = _oneLine(item.versionNote ?? '');
      final v = n == null ? 'New version' : 'v$n';
      return note.isEmpty ? v : '$v: $note';
    case 'published':
      return 'New app';
    case 'question':
      final q = item.question;
      if (q == null || q.questions.isEmpty) return 'A question';
      final header = _oneLine(q.questions.first.header);
      return header.isEmpty ? _oneLine(q.questions.first.question) : header;
    case 'finished':
      final m = _oneLine(item.workMessage ?? '');
      return m.isEmpty ? 'Finished' : 'Finished: $m';
    default:
      return item.kind;
  }
}

/// [text] with every run of whitespace, newlines included, as one space.
String _oneLine(String text) => text.replaceAll(RegExp(r'\s+'), ' ').trim();

/// Markdown [text] as one line of plain text for a preview: line markers
/// (headings, quotes, list bullets and numbers, fences) and inline markers
/// (emphasis, code, links' and images' syntax) dropped, whitespace
/// collapsed. Blocks (paragraphs, headings, list items) are joined with
/// ` · ` when the earlier one ends without punctuation, else a space.
String plainPreview(String text) {
  final marker = RegExp(
    r'^\s*(?:#{1,6}\s+|>\s?|[-*+]\s+(?:\[[ xX]\]\s+)?|\d+[.)]\s+)*',
  );
  final ownBlock = RegExp(r'^\s*(?:#{1,6}\s|[-*+]\s|\d+[.)]\s)');
  final blocks = <String>[];
  var current = <String>[];
  void endBlock() {
    final b = _oneLine(current.join(' '));
    if (b.isNotEmpty) blocks.add(b);
    current = [];
  }

  for (final line in text.split('\n')) {
    if (RegExp(r'^\s*(```|~~~)').hasMatch(line)) continue;
    if (line.trim().isEmpty || ownBlock.hasMatch(line)) endBlock();
    current.add(line.replaceFirst(marker, ''));
    if (RegExp(r'^\s*#{1,6}\s').hasMatch(line)) endBlock();
  }
  endBlock();
  final lines = <String>[];
  for (final b in blocks) {
    if (lines.isNotEmpty && !RegExp(r'[.!?:;,…]$').hasMatch(lines.last)) {
      lines.add('·');
    }
    lines.add(b);
  }
  final plain = lines
      .join(' ')
      .replaceAllMapped(RegExp(r'!?\[([^\]]*)\]\([^)]*\)'), (m) => m[1]!)
      .replaceAllMapped(RegExp(r'(\*\*|__)(.+?)\1'), (m) => m[2]!)
      .replaceAllMapped(
        RegExp(r'(?<![\w*])\*(?!\s)(.+?)(?<!\s)\*(?![\w*])'),
        (m) => m[1]!,
      )
      .replaceAllMapped(
        RegExp(r'(?<!\w)_(?!\s)(.+?)(?<!\s)_(?!\w)'),
        (m) => m[1]!,
      )
      .replaceAllMapped(RegExp(r'~~(.+?)~~'), (m) => m[1]!)
      .replaceAllMapped(RegExp(r'`+([^`]*)`+'), (m) => m[1]!);
  return _oneLine(plain);
}

/// Subscribes [stream] to [topics] again after the daemon dropped them
/// (an `unsubscribed` event): at once, then after [delay] of 1 s doubling
/// to 60 s between tries. True once subscribed; false when [alive] turns
/// false or the stream is closed.
Future<bool> resubscribe(
  ClaxStream stream,
  List<String> topics, {
  required Future<void> Function(Duration) delay,
  required bool Function() alive,
}) async {
  const most = Duration(seconds: 60);
  var wait = const Duration(seconds: 1);
  while (alive()) {
    try {
      await stream.subscribe(topics);
      return true;
    } on StateError {
      return false;
    } catch (_) {
      // Refused or unanswered: try again after the wait.
    }
    await delay(wait);
    wait = wait * 2 > most ? most : wait * 2;
  }
  return false;
}

/// The icon of an item's kind.
IconData inboxIcon(String kind) => switch (kind) {
  'reply' => Icons.chat_bubble_rounded,
  'version' => Icons.history_rounded,
  'published' => Icons.auto_awesome_rounded,
  'question' => Icons.help_rounded,
  'finished' => Icons.task_alt_rounded,
  _ => Icons.notifications_rounded,
};

const _months = [
  'Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', //
  'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec',
];
const _days = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];

/// [createdAt] (RFC 3339) as a row shows it next to [now]: `now`, `5m`,
/// `3h`, the weekday within a week, else `Sep 3` (with the year when it is
/// not [now]'s). Empty when [createdAt] does not parse.
String inboxTime(String createdAt, DateTime now) {
  final t = DateTime.tryParse(createdAt);
  if (t == null) return '';
  final at = t.toLocal();
  final ago = now.difference(t);
  if (ago.inMinutes < 1) return 'now';
  if (ago.inHours < 1) return '${ago.inMinutes}m';
  if (ago.inHours < 24) return '${ago.inHours}h';
  if (ago.inDays < 7) return _days[at.weekday - 1];
  final day = '${_months[at.month - 1]} ${at.day}';
  return at.year == now.toLocal().year ? day : '$day, ${at.year}';
}

/// One row of the inbox: a badge in the app's colour with the kind's icon,
/// the app's title, the kind's line, the time and, while unread, a dot.
class InboxRow extends StatelessWidget {
  const InboxRow({
    super.key,
    required this.item,
    required this.now,
    required this.onTap,
  });

  final InboxItem item;
  final DateTime now;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final unread = !item.read;
    final aid = item.artifactId;
    final tile = aid == null ? scheme : tileScheme(aid, theme.brightness);
    final title = item.gone
        ? 'Deleted'
        : (item.artifactTitle?.trim().isNotEmpty ?? false)
        ? item.artifactTitle!.trim()
        : 'Clax';
    final line = inboxLine(item);
    final time = inboxTime(item.createdAt, now);
    return Semantics(
      button: true,
      label: [if (unread) 'Unread', title, line, time].join('. '),
      excludeSemantics: true,
      child: Material(
        color: unread
            ? scheme.secondaryContainer.withValues(alpha: 0.35)
            : Colors.transparent,
        child: InkWell(
          onTap: onTap,
          child: ConstrainedBox(
            constraints: const BoxConstraints(minHeight: 76),
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 12, 12, 12),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Container(
                    width: 44,
                    height: 44,
                    decoration: BoxDecoration(
                      color: item.gone
                          ? scheme.surfaceContainerHighest
                          : tile.primaryContainer,
                      borderRadius: BorderRadius.circular(14),
                    ),
                    alignment: Alignment.center,
                    child: Icon(
                      inboxIcon(item.kind),
                      size: 22,
                      color: item.gone
                          ? scheme.onSurfaceVariant
                          : tile.onPrimaryContainer,
                    ),
                  ),
                  const SizedBox(width: 14),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Row(
                          crossAxisAlignment: CrossAxisAlignment.baseline,
                          textBaseline: TextBaseline.alphabetic,
                          children: [
                            Expanded(
                              child: Text(
                                title,
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: theme.textTheme.titleMedium?.copyWith(
                                  fontWeight: unread
                                      ? FontWeight.w700
                                      : FontWeight.w500,
                                ),
                              ),
                            ),
                            const SizedBox(width: 8),
                            Text(
                              time,
                              style: theme.textTheme.labelMedium?.copyWith(
                                color: unread
                                    ? scheme.primary
                                    : scheme.onSurfaceVariant,
                                fontWeight: unread ? FontWeight.w700 : null,
                              ),
                            ),
                          ],
                        ),
                        const SizedBox(height: 2),
                        Row(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Expanded(
                              child: Text(
                                line,
                                maxLines: 2,
                                overflow: TextOverflow.ellipsis,
                                style: theme.textTheme.bodyMedium?.copyWith(
                                  color: unread
                                      ? scheme.onSurface
                                      : scheme.onSurfaceVariant,
                                  fontStyle: item.gone
                                      ? FontStyle.italic
                                      : null,
                                ),
                              ),
                            ),
                            SizedBox(
                              width: 20,
                              height: 20,
                              child: unread
                                  ? Center(
                                      child: Container(
                                        width: 10,
                                        height: 10,
                                        decoration: BoxDecoration(
                                          color: scheme.primary,
                                          shape: BoxShape.circle,
                                        ),
                                      ),
                                    )
                                  : null,
                            ),
                          ],
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _PageEnd extends StatelessWidget {
  const _PageEnd({required this.failed, required this.onRetry});

  final bool failed;
  final VoidCallback onRetry;

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: 72,
      child: Center(
        child: failed
            ? TextButton.icon(
                onPressed: onRetry,
                icon: const Icon(Icons.refresh_rounded),
                label: const Text('Load more'),
              )
            : const SizedBox.square(
                dimension: 24,
                child: CircularProgressIndicator(strokeWidth: 2.5),
              ),
      ),
    );
  }
}

