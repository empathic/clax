import 'dart:async';

import 'package:flutter/material.dart';

import 'kit/kit.dart';
import 'theme.dart';
import 'tokens.g.dart';

/// The change in progress as a [CommentTab]'s menu offers it: its
/// [label], and [onOpen], which shows it.
typedef CommentTabChange = ({String label, VoidCallback onOpen});

/// The Android Comment tab, the way into comment mode where the open app's
/// notice cannot be seen: a labelled tab, "Comment" with the 16 dp mark,
/// 44 dp high in a 48 dp target, `fg` filled with `bg` words at level 2.
///
/// It fills its parent and sits at its right, its right edge 4 dp inside
/// the system back-gesture zone
/// (`MediaQuery.systemGestureInsetsOf(context).right + 4` from the edge),
/// so no back swipe starts on it at any gesture sensitivity; its centre is
/// at [at] of the parent's height, kept inside the parent's safe area.
///
/// It is tap-only: a tap opens a [ClaxMenu] of Comment ([onComment]),
/// "Threads (n)" with [threads] open ([onThreads]) and, while one runs,
/// [change]. A long-press and then a vertical drag move it; as the press
/// ends [onMoved] gets the new fraction of the height. A drag without the
/// long-press does nothing.
class CommentTab extends StatefulWidget {
  const CommentTab({
    super.key,
    required this.threads,
    required this.onComment,
    required this.onThreads,
    this.change,
    required this.at,
    required this.onMoved,
  });

  /// How many threads are open, as the menu's Threads says.
  final int threads;
  final VoidCallback onComment, onThreads;

  /// The change in progress; null while none runs.
  final CommentTabChange? change;

  /// The tab's centre, as a fraction of the parent's height.
  final double at;

  /// A move ended at this fraction of the height, for the caller to keep.
  final ValueChanged<double> onMoved;

  /// Where the tab sits until it is moved.
  static const defaultAt = 0.62;

  /// How tall the tab is to see; its target is [ClaxLayout.target].
  static const height = 44.0;

  @override
  State<CommentTab> createState() => _CommentTabState();
}

class _CommentTabState extends State<CommentTab> {
  late double _at = widget.at;
  final _key = GlobalKey();

  /// While a move goes on: the centre where the press started.
  double? _from;

  @override
  void didUpdateWidget(CommentTab old) {
    super.didUpdateWidget(old);
    if (old.at != widget.at && _from == null) _at = widget.at;
  }

  Future<void> _menu() async {
    final box = _key.currentContext?.findRenderObject() as RenderBox?;
    if (box == null) return;
    final anchor = box.localToGlobal(Offset.zero) & box.size;
    final change = widget.change;
    final id = await ClaxMenu.show(
      context,
      anchor: anchor,
      items: [
        const ClaxMenuItem(
          'comment',
          'Comment',
          icon: Icons.add_comment_outlined,
          symbol: 'text.bubble',
        ),
        ClaxMenuItem(
          'threads',
          'Threads (${widget.threads})',
          icon: Icons.forum_outlined,
          symbol: 'bubble.left.and.bubble.right',
        ),
        if (change != null)
          ClaxMenuItem(
            'change',
            change.label,
            icon: Icons.autorenew_rounded,
            symbol: 'arrow.triangle.2.circlepath',
          ),
      ],
    );
    switch (id) {
      case 'comment':
        widget.onComment();
      case 'threads':
        widget.onThreads();
      case 'change':
        change?.onOpen();
    }
  }

  @override
  Widget build(BuildContext context) => LayoutBuilder(
    builder: (context, constraints) {
      final height = constraints.maxHeight;
      final pad = MediaQuery.paddingOf(context);
      const half = ClaxLayout.target / 2;
      final top = pad.top + half;
      final bottom = height - pad.bottom - half;
      double clamp(double y) => bottom <= top ? height / 2 : y.clamp(top, bottom);
      final centre = clamp(_at * height);
      final right = MediaQuery.systemGestureInsetsOf(context).right + 4;
      return Stack(
        children: [
          Positioned(
            right: right,
            top: centre - half,
            child: _tab(context, height, clamp),
          ),
        ],
      );
    },
  );

  Widget _tab(
    BuildContext context,
    double height,
    double Function(double) clamp,
  ) {
    final p = claxPalette(context);
    final theme = Theme.of(context);
    final face = DecoratedBox(
      key: const ValueKey('comment-tab-face'),
      decoration: ClaxElevation.of(
        2,
        theme.brightness,
        p,
        color: p.fg,
        radius: BorderRadius.circular(ClaxRadius.full),
      ),
      child: ConstrainedBox(
        constraints: const BoxConstraints(minHeight: CommentTab.height),
        child: Padding(
          padding: const EdgeInsetsDirectional.fromSTEB(12, 4, 16, 4),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              const ClaxMark(size: 16, inverse: true),
              const SizedBox(width: ClaxSpace.s8),
              Text(
                'Comment',
                maxLines: 1,
                softWrap: false,
                style: theme.textTheme.labelLarge!.copyWith(color: p.bg),
              ),
            ],
          ),
        ),
      ),
    );
    return Semantics(
      container: true,
      button: true,
      label: 'Comment',
      onTap: () => unawaited(_menu()),
      excludeSemantics: true,
      child: GestureDetector(
        key: const ValueKey('comment-tab'),
        behavior: HitTestBehavior.opaque,
        onTap: () => unawaited(_menu()),
        onLongPressStart: (_) => _from = clamp(_at * height),
        onLongPressMoveUpdate: (d) {
          final from = _from;
          if (from == null || height <= 0) return;
          setState(() => _at = clamp(from + d.offsetFromOrigin.dy) / height);
        },
        onLongPressEnd: (_) {
          final from = _from;
          _from = null;
          if (from == null || height <= 0) return;
          final at = clamp(_at * height) / height;
          if (at != from / height) widget.onMoved(at);
        },
        child: ConstrainedBox(
          key: _key,
          constraints: const BoxConstraints(
            minWidth: ClaxLayout.target,
            minHeight: ClaxLayout.target,
          ),
          child: Align(
            alignment: AlignmentDirectional.centerEnd,
            widthFactor: 1,
            heightFactor: 1,
            child: face,
          ),
        ),
      ),
    );
  }
}
