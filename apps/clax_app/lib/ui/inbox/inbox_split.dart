import 'dart:async';

import 'package:flutter/material.dart';

import '../../clax/api.dart';
import '../../platform/voice.dart';
import '../app_icon.dart';
import '../inbox_screen.dart';
import '../kit/kit.dart';
import '../layout.dart';
import '../mac/mac_toolbar.dart';
import '../mac/window_channel.dart';
import '../theme.dart';
import '../thread_detail.dart';
import '../tokens.g.dart';

/// Opens app [aid], at [thread] (and [comment] in it) or [version] when
/// given.
typedef OpenAppAt =
    void Function(String aid, {String? thread, int? version, String? comment});

/// The inbox as a list and the item beside it, by size class: from
/// medium width a 360 point list pane ([InboxScreen] as a pane) with the
/// selected item shown beside it, a reply as its thread ([ThreadDetail]),
/// anything else as what it is with Open app; below that the list alone,
/// a row pushing its item over it. A question goes to [onOpenQuestion],
/// when given, as on the phone.
///
/// With [toolbar] (the Mac's Inbox window) the window's toolbar carries
/// All, Unread and Questions as one segmented control, the search field
/// and Mark all read; the content starts below the toolbar row. Every Open
/// app goes to [onOpenApp], which brings the main window forward with that
/// app. [onClosed] runs when it goes.
class InboxSplit extends StatefulWidget {
  const InboxSplit({
    super.key,
    required this.api,
    required this.stream,
    required this.onOpenApp,
    required this.voice,
    this.onOpenQuestion,
    this.icons,
    this.autoDictate = false,
    this.now,
    this.toolbar = false,
    this.onClosed,
  });

  final ClaxApi api;

  /// The stream the list follows, which it leaves open.
  final ClaxStream stream;
  final OpenAppAt onOpenApp;
  final VoiceInput voice;
  final void Function(String qid)? onOpenQuestion;
  final AppIcons? icons;

  /// Whether a thread's reply box starts dictation when focused.
  final bool autoDictate;

  /// The clock the times are relative to; [DateTime.now] when null.
  final DateTime Function()? now;

  /// Whether the window's toolbar holds the search and the chips.
  final bool toolbar;
  final VoidCallback? onClosed;

  /// The chips the Mac's segmented control offers.
  static const macFilters = [
    InboxFilter.all,
    InboxFilter.unread,
    InboxFilter.questions,
  ];

  /// The list pane's width beside the item.
  static const listWidth = 360.0;

  @override
  State<InboxSplit> createState() => _InboxSplitState();
}

class _InboxSplitState extends State<InboxSplit> {
  InboxItem? _shown;
  final _list = GlobalKey();
  final _query = ValueNotifier(const InboxQuery());
  StreamSubscription<ToolbarClick>? _clicks;
  StreamSubscription<ToolbarSearch>? _searches;
  int? _view;

  bool get _toolbar => widget.toolbar && hasDesktopWindowChrome;

  @override
  void initState() {
    super.initState();
    if (_toolbar) {
      _clicks = MacToolbar.clicks.listen(_clicked);
      _searches = MacToolbar.searches.listen(_searched);
    }
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    if (!_toolbar) return;
    final view = View.of(context).viewId;
    if (view == _view) return;
    _view = view;
    _declare();
    unawaited(MacWindow.setTitle('Inbox', view: view));
  }

  @override
  void dispose() {
    unawaited(_clicks?.cancel());
    unawaited(_searches?.cancel());
    if (_view case final view?) MacToolbar.clear(this, view: view);
    _query.dispose();
    widget.onClosed?.call();
    super.dispose();
  }

  static const _symbols = {
    InboxFilter.all: 'tray',
    InboxFilter.unread: 'envelope.badge',
    InboxFilter.questions: 'questionmark.bubble',
  };

  void _declare() {
    final view = _view;
    if (view == null) return;
    final filter = _query.value.filter;
    MacToolbar.setItems(this, [
      for (final f in InboxSplit.macFilters)
        ToolbarItemSpec(
          f.name,
          symbol: _symbols[f]!,
          label: f.label,
          group: 'filter',
          select: true,
          on: f == filter,
        ),
      const ToolbarItemSpec(
        'markAllRead',
        symbol: 'checkmark.circle',
        label: 'Mark all read',
      ),
      const ToolbarItemSpec(
        'search',
        symbol: 'magnifyingglass',
        label: 'Search the inbox',
        search: true,
      ),
    ], view: view);
  }

  void _clicked(ToolbarClick c) {
    if (!mounted || c.view != _view) return;
    if (c.id == 'markAllRead') {
      unawaited((_list.currentState as InboxMarking?)?.markAllRead());
      return;
    }
    final f = InboxSplit.macFilters.where((f) => f.name == c.id).firstOrNull;
    if (f == null || f == _query.value.filter) return;
    _query.value = _query.value.copyWith(filter: f);
    _declare();
  }

  void _searched(ToolbarSearch s) {
    if (!mounted || s.view != _view || s.id != 'search') return;
    _query.value = _query.value.copyWith(text: s.text);
  }

  void _open(InboxItem item, {required bool split}) {
    final question = item.question;
    if (item.kind == 'question' &&
        question != null &&
        widget.onOpenQuestion != null) {
      widget.onOpenQuestion!(question.id);
      return;
    }
    if (split) {
      setState(() => _shown = item);
      return;
    }
    unawaited(
      Navigator.of(
        context,
      ).push(MaterialPageRoute<void>(builder: (_) => _detail(item))),
    );
  }

  /// Opens [item]'s app at what it points to.
  void _openApp(InboxItem item) {
    final aid = item.artifactId;
    if (aid == null) return;
    widget.onOpenApp(
      aid,
      thread: item.kind == 'reply' ? item.thread?.id : null,
      version: item.kind == 'version' ? item.versionN : null,
      comment: item.kind == 'reply' ? item.replyCommentId : null,
    );
  }

  Widget _detail(InboxItem item) {
    final aid = item.artifactId;
    final thread = item.thread;
    if (item.kind == 'reply' && aid != null && thread != null) {
      return ThreadDetail(
        key: ValueKey('inbox-thread-${thread.id}'),
        api: widget.api,
        aid: aid,
        tid: thread.id,
        voice: widget.voice,
        autoDictate: widget.autoDictate,
        appTitle: item.artifactTitle,
        focusComment: item.replyCommentId,
        focusLatestReply: true,
        now: widget.now ?? DateTime.now,
        onPreview: (n) => widget.onOpenApp(aid, version: n),
        onRetry: null,
        onOpenApp: () => _openApp(item),
      );
    }
    return _ItemDetail(
      key: ValueKey('inbox-item-${item.id}'),
      item: item,
      now: (widget.now ?? DateTime.now)(),
      icons: widget.icons,
      onOpenApp: aid == null ? null : () => _openApp(item),
    );
  }

  Widget _listPane({required bool split}) => InboxScreen(
    key: _list,
    api: widget.api,
    stream: widget.stream,
    icons: widget.icons,
    now: widget.now,
    pane: split || _toolbar,
    selected: split ? _shown?.id : null,
    query: _toolbar ? _query : null,
    filters: hasDesktopWindowChrome
        ? InboxSplit.macFilters
        : InboxFilter.values,
    onOpen: (item) => _open(item, split: split),
  );

  @override
  Widget build(BuildContext context) {
    final split = ClaxWidth.of(context) >= ClaxWidth.medium;
    final p = claxPalette(context);
    Widget body;
    if (!split) {
      body = _listPane(split: false);
    } else {
      final shown = _shown;
      body = Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          SizedBox(
            key: const ValueKey('inbox-list-pane'),
            width: InboxSplit.listWidth,
            child: _listPane(split: true),
          ),
          VerticalDivider(width: 1, thickness: 1, color: p.border),
          Expanded(
            child: shown == null
                ? Material(
                    color: p.bg,
                    child: const ClaxEmpty(
                      key: ValueKey('inbox-nothing-selected'),
                      icon: Icons.inbox_outlined,
                      title: 'Nothing selected',
                      body: 'Choose a reply or a change to read it here.',
                    ),
                  )
                : _detail(shown),
          ),
        ],
      );
    }
    if (!_toolbar) return body;
    // Below the window's toolbar row, which the content runs under.
    final top = MediaQuery.paddingOf(context).top;
    return ColoredBox(
      color: p.bg,
      child: Padding(
        padding: EdgeInsets.only(top: top),
        child: MediaQuery.removePadding(
          context: context,
          removeTop: true,
          child: body,
        ),
      ),
    );
  }
}

/// An item that is no thread: its app, what it says, when, and Open app.
class _ItemDetail extends StatelessWidget {
  const _ItemDetail({
    super.key,
    required this.item,
    required this.now,
    required this.onOpenApp,
    this.icons,
  });

  final InboxItem item;
  final DateTime now;
  final VoidCallback? onOpenApp;
  final AppIcons? icons;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final title = (item.artifactTitle?.trim().isNotEmpty ?? false)
        ? item.artifactTitle!.trim()
        : 'Clax';
    return ClaxScaffold(
      title: title,
      body: Align(
        alignment: Alignment.topCenter,
        child: ConstrainedBox(
          constraints: const BoxConstraints(maxWidth: ClaxLayout.reading),
          child: ListView(
            padding: const EdgeInsets.all(ClaxSpace.s24),
            children: [
              Text(
                inboxLine(item),
                style: ClaxType.of(
                  ClaxRole.title3,
                  theme.platform,
                ).copyWith(color: p.fg),
              ),
              const SizedBox(height: ClaxSpace.s8),
              Text(
                inboxTime(item.createdAt, now),
                style: theme.textTheme.bodySmall?.copyWith(color: p.muted),
              ),
              if (onOpenApp case final open?) ...[
                const SizedBox(height: ClaxSpace.s20),
                Align(
                  alignment: AlignmentDirectional.centerStart,
                  child: ClaxButton(
                    label: 'Open app',
                    kind: ClaxButtonKind.tonal,
                    onPressed: open,
                  ),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}
