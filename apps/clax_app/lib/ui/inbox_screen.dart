import 'dart:async';
import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../clax/api.dart';
import 'app_icon.dart';
import 'haiku.dart';
import 'kit/kit.dart';
import 'layout.dart';
import 'theme.dart';
import 'tokens.g.dart';
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
/// The chips (one of All, Unread, Questions and Changes, [InboxFilter])
/// and the search bar (after a pause in typing of [searchDelay]) change the
/// query; scrolling to the end fetches the next page with `before`. A new
/// item that arrives live is added when it matches the chip; while a
/// search is active only items already shown are updated, since the daemon
/// decides what a search matches.
///
/// A tap marks the item read and passes it to [onOpen]; an item whose
/// source is gone is only marked read. **Mark all read** marks every unread
/// item matching the current filter up to the newest one shown.
///
/// With nothing left to read it says "All caught up." over a haiku, a new
/// one each visit; a long-press (Option-click on the Mac) shows another.
///
/// As a [pane] (the list of an `InboxSplit`) it is the list under a top
/// app bar ("Inbox" and Mark all read, as tall as the item's bar beside
/// it), its search and chips, with the [selected] item's row highlighted;
/// with a [query] from elsewhere (the Mac Inbox window's toolbar) it draws
/// no bar, search or chips and follows that query instead, the text after
/// the same pause in typing.
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
    this.icons,
    this.pane = false,
    this.selected,
    this.query,
    this.filters = InboxFilter.values,
  });

  /// Whether it is a list pane beside the item shown, not a screen.
  final bool pane;

  /// The ID of the item shown beside the pane, whose row is highlighted.
  final String? selected;

  /// The chip and the search text, set from elsewhere; the screen then
  /// draws neither.
  final ValueListenable<InboxQuery>? query;

  /// The chips offered, in order.
  final List<InboxFilter> filters;

  final ClaxApi api;

  /// The apps' icons, drawn on each row; null draws each app's tile face.
  final AppIcons? icons;

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

/// The inbox's views, one chip each: everything, the unread, Clax's
/// questions, and the changes Clax made (new versions, new apps and
/// finished work).
enum InboxFilter {
  all('All'),
  unread('Unread'),
  questions('Questions', ['question']),
  changes('Changes', ['version', 'published', 'finished']);

  const InboxFilter(this.label, [this.kinds]);

  /// The chip's label.
  final String label;

  /// The item kinds the view shows; null for every kind.
  final List<String>? kinds;
}

/// What an inbox shows, when it is chosen outside it: the chip, and the
/// search text.
@immutable
final class InboxQuery {
  const InboxQuery({this.filter = InboxFilter.all, this.text = ''});

  final InboxFilter filter;
  final String text;

  InboxQuery copyWith({InboxFilter? filter, String? text}) =>
      InboxQuery(filter: filter ?? this.filter, text: text ?? this.text);

  @override
  bool operator ==(Object other) =>
      other is InboxQuery && other.filter == filter && other.text == text;

  @override
  int get hashCode => Object.hash(filter, text);
}

/// What an inbox does for a window's toolbar.
abstract interface class InboxMarking {
  /// Marks every unread item matching the current view read.
  Future<void> markAllRead();
}

class _InboxScreenState extends State<InboxScreen> implements InboxMarking {
  ClaxStream? _stream;
  StreamSubscription<StreamEvent>? _events;

  List<InboxItem>? _items;
  String? _next;
  Object? _loadError;
  bool _pageError = false;
  bool _paging = false;

  InboxFilter _filter = InboxFilter.all;
  bool get _unreadOnly => _filter == InboxFilter.unread;
  List<String>? get _kinds => _filter.kinds;
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
    if (widget.query case final q?) {
      _filter = q.value.filter;
      _query = q.value.text.trim();
      _search.text = q.value.text;
      q.addListener(_queryChanged);
    }
    unawaited(_start());
    unawaited(_loadHaikus());
  }

  /// The query set from outside changed: a chip at once, text after the
  /// pause in typing.
  void _queryChanged() {
    final q = widget.query!.value;
    if (q.filter != _filter) {
      _searchTimer?.cancel();
      _setFilter(() {
        _filter = q.filter;
        _query = q.text.trim();
      });
      return;
    }
    _onSearch(q.text);
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
    widget.query?.removeListener(_queryChanged);
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
        kinds: _kinds == null ? null : List.of(_kinds!),
        read: _read,
      );
      if (!mounted || load != _loads) return;
      final late = _duringLoad ?? const [];
      _duringLoad = null;
      setState(() {
        _items = page.items;
        _next = page.nextCursor;
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
        kinds: _kinds == null ? null : List.of(_kinds!),
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
    if (_kinds case final kinds? when !kinds.contains(item.kind)) return;
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
    widget.api.markInboxRead(item.id).ignore();
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

  @override
  Future<void> markAllRead() => _markAllRead();

  Future<void> _markAllRead() async {
    final items = _items;
    if (items == null || items.isEmpty) return;
    final upto = items.map((i) => i.seq).reduce((a, b) => a > b ? a : b);
    final filter = <String, Object?>{
      if (_query.isNotEmpty) 'q': _query,
      if (_kinds case final kinds?) 'kind': List.of(kinds),
    };
    setState(() {
      _items = [for (final i in items) i.seq <= upto ? _asRead(i) : i];
    });
    try {
      await widget.api.markAllInboxRead(
        upto: upto,
        filter: filter.isEmpty ? null : filter,
      );
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
    if (widget.pane) {
      return Material(
        color: claxPalette(context).bg,
        child: RefreshIndicator(
          onRefresh: _refresh,
          child: CustomScrollView(
            slivers: [
              // Below the status bar, where a window has one.
              SliverToBoxAdapter(
                child: SizedBox(height: MediaQuery.paddingOf(context).top),
              ),
              if (widget.query == null) ...[
                SliverToBoxAdapter(child: _paneBar(context, anyUnread)),
                SliverToBoxAdapter(child: _filters(context)),
              ],
              ..._content(items, now),
            ],
          ),
        ),
      );
    }
    return RefreshIndicator(
      onRefresh: _refresh,
      edgeOffset: 152,
      child: ClaxScaffold(
        title: 'Inbox',
        largeTitle: true,
        contentWidth: ClaxLayout.reading,
        actions: [
          IconButton(
            tooltip: 'Mark all read',
            onPressed: anyUnread ? _markAllRead : null,
            icon: const Icon(Icons.done_all_rounded),
          ),
        ],
        slivers: [
          SliverToBoxAdapter(child: _filters(context)),
          ..._content(items, now),
        ],
      ),
    );
  }

  /// A list pane's top app bar, as tall as the item's beside it: "Inbox"
  /// and Mark all read.
  Widget _paneBar(BuildContext context, bool anyUnread) {
    final theme = Theme.of(context);
    return SizedBox(
      key: const ValueKey('inbox-pane-bar'),
      // Material 3's small top app bar.
      height: 64,
      child: Padding(
        padding: EdgeInsetsDirectional.only(
          start: ClaxBreakpoints.gutter(context),
          end: ClaxSpace.s4,
        ),
        child: Row(
          children: [
            Expanded(
              child: Text(
                'Inbox',
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: theme.appBarTheme.titleTextStyle ??
                    theme.textTheme.titleLarge,
              ),
            ),
            IconButton(
              tooltip: 'Mark all read',
              onPressed: anyUnread ? _markAllRead : null,
              icon: const Icon(Icons.done_all_rounded),
            ),
          ],
        ),
      ),
    );
  }

  /// The list, or what shows in its place: the error, the wait, the empty
  /// state.
  List<Widget> _content(List<InboxItem>? items, DateTime now) => [
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
        padding: const EdgeInsets.only(bottom: ClaxSpace.s24),
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
              icons: widget.icons,
              selected: item.id == widget.selected,
              onTap: () => _tap(item),
            );
          },
        ),
      ),
  ];

  Widget _filters(BuildContext context) {
    final gutter = ClaxBreakpoints.gutter(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: EdgeInsets.fromLTRB(gutter, 0, gutter, ClaxSpace.s12),
          child: SearchBar(
            controller: _search,
            onChanged: _onSearch,
            onSubmitted: (_) {
              _searchTimer?.cancel();
              _commitSearch();
            },
            textInputAction: TextInputAction.search,
            hintText: 'Search the inbox',
            elevation: const WidgetStatePropertyAll(0),
            constraints: const BoxConstraints(minHeight: 56, maxHeight: 56),
            shape: const WidgetStatePropertyAll(
              RoundedRectangleBorder(
                borderRadius: BorderRadius.all(Radius.circular(ClaxRadius.xl)),
              ),
            ),
            padding: const WidgetStatePropertyAll(
              EdgeInsets.symmetric(horizontal: ClaxSpace.s16),
            ),
            leading: const Icon(Icons.search_rounded),
            trailing: [
              ListenableBuilder(
                listenable: _search,
                builder: (context, _) => _search.text.isEmpty
                    ? const SizedBox.shrink()
                    : IconButton(
                        tooltip: 'Clear search',
                        onPressed: _clearSearch,
                        icon: const Icon(Icons.close_rounded),
                      ),
              ),
            ],
          ),
        ),
        SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          padding: EdgeInsets.fromLTRB(gutter, 0, gutter, ClaxSpace.s8),
          child: Row(
            children: [
              for (final f in widget.filters) ...[
                FilterChip(
                  label: Text(f.label),
                  selected: _filter == f,
                  showCheckmark: false,
                  onSelected: (_) {
                    if (_filter != f) _setFilter(() => _filter = f);
                  },
                ),
                if (f != widget.filters.last)
                  const SizedBox(width: ClaxSpace.s8),
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
    if (_query.isNotEmpty || _kinds != null) {
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

/// The line under an item's title, naming its kind in words: "Clax
/// replied: …" with the reply, "Version 5 is ready: …" with its note, "New
/// app: …" with its description, "Clax asked: …" with the question, or
/// "Clax finished: …" with its message.
String inboxLine(InboxItem item) {
  if (item.gone) return 'No longer available';
  String said(String lead, String text) =>
      text.isEmpty ? lead : '$lead: $text';
  switch (item.kind) {
    case 'reply':
      return said('Clax replied', plainPreview(item.replyBody ?? ''));
    case 'version':
      final n = item.versionN;
      return said(
        n == null ? 'A new version is ready' : 'Version $n is ready',
        _oneLine(item.versionNote ?? ''),
      );
    case 'published':
      return said('New app', _oneLine(item.publishedDescription ?? ''));
    case 'question':
      final q = item.question?.questions.firstOrNull;
      final text = _oneLine(q?.question ?? '');
      return said('Clax asked', text.isEmpty ? _oneLine(q?.header ?? '') : text);
    case 'finished':
      return said('Clax finished', _oneLine(item.workMessage ?? ''));
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

/// One row of the inbox: the app's 40 dp icon, its title in `headline`,
/// the kind's line ([inboxLine]) and the time trailing. Unread is a bold
/// title and an 8 dp dot in `you`; no row is tinted for it. The row shown
/// beside a list pane ([selected]) is filled in a raised surface. A row
/// under the mouse fills with `hover` and shows the arrow (an [InkWell]'s
/// cursor off the web).
class InboxRow extends StatelessWidget {
  const InboxRow({
    super.key,
    required this.item,
    required this.now,
    required this.onTap,
    this.icons,
    this.selected = false,
  });

  final InboxItem item;
  final DateTime now;
  final VoidCallback onTap;

  /// Whether the item is the one shown beside the list.
  final bool selected;

  /// The apps' icons; null draws the app's tile face.
  final AppIcons? icons;

  static const _icon = 40.0;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final unread = !item.read;
    final title = item.gone
        ? 'Deleted'
        : (item.artifactTitle?.trim().isNotEmpty ?? false)
        ? item.artifactTitle!.trim()
        : 'Clax';
    final line = inboxLine(item);
    final time = inboxTime(item.createdAt, now);
    final headline = ClaxType.of(ClaxRole.headline, theme.platform);
    final gutter = ClaxBreakpoints.gutter(context);
    return Semantics(
      button: true,
      selected: selected,
      label: [if (unread) 'Unread', title, line, time].join('. '),
      excludeSemantics: true,
      child: Material(
        color: selected ? p.surfaceContainers[3] : Colors.transparent,
        child: InkWell(
        onTap: onTap,
        hoverColor: p.hover,
        child: ConstrainedBox(
          constraints: const BoxConstraints(minHeight: 72),
          child: Padding(
            padding: EdgeInsets.symmetric(
              horizontal: gutter,
              vertical: ClaxSpace.s12,
            ),
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                _badge(context),
                const SizedBox(width: ClaxSpace.s16),
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(
                        title,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: headline.copyWith(
                          color: p.fg,
                          fontWeight: unread
                              ? FontWeight.w700
                              : FontWeight.w500,
                        ),
                      ),
                      const SizedBox(height: ClaxSpace.s2),
                      Text(
                        line,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: theme.textTheme.bodyMedium?.copyWith(
                          color: unread ? p.fg : p.muted,
                          fontStyle: item.gone ? FontStyle.italic : null,
                        ),
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: ClaxSpace.s12),
                Column(
                  crossAxisAlignment: CrossAxisAlignment.end,
                  children: [
                    Text(
                      time,
                      style: ClaxType.of(
                        ClaxRole.caption,
                        theme.platform,
                      ).copyWith(color: unread ? p.fg : p.muted),
                    ),
                    if (unread) ...[
                      const SizedBox(height: ClaxSpace.s8),
                      Container(
                        key: const ValueKey('inbox-unread-dot'),
                        width: 8,
                        height: 8,
                        decoration: BoxDecoration(
                          color: p.you,
                          shape: BoxShape.circle,
                        ),
                      ),
                    ],
                  ],
                ),
              ],
            ),
          ),
        ),
        ),
      ),
    );
  }

  /// The app's icon; a neutral square for an item whose app is gone, and
  /// the mark for one from no app.
  Widget _badge(BuildContext context) {
    final p = claxPalette(context);
    final aid = item.artifactId;
    final radius = ClaxRadius.tile(_icon);
    if (item.gone || aid == null) {
      return Container(
        width: _icon,
        height: _icon,
        decoration: BoxDecoration(
          color: p.card,
          border: Border.all(color: p.border),
          borderRadius: BorderRadius.circular(radius),
        ),
        alignment: Alignment.center,
        child: item.gone
            ? Icon(Icons.delete_outline_rounded, size: 20, color: p.muted)
            : const ClaxMark(size: 20),
      );
    }
    final app = ArtifactSummary(
      id: aid,
      title: item.artifactTitle ?? '',
      currentVersion: item.versionN ?? 0,
      updatedAt: item.createdAt,
      kind: item.artifactKind ?? 'html',
    );
    final icons = this.icons;
    Widget badge(String? svg) =>
        AppBadge(app: app, svg: svg, size: _icon, radius: radius);
    if (icons == null) return badge(null);
    return ListenableBuilder(
      listenable: icons,
      builder: (context, _) => badge(icons.svgOf(aid)),
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

