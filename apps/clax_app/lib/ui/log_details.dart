import 'package:flutter/material.dart';

import 'kit/kit.dart';
import 'theme.dart';
import 'tokens.g.dart';

/// Raw text, such as a log or an error's own words, kept under a Details
/// disclosure: closed, a Details button; open, [text] in `mono` on a
/// `card` panel the column's width, its lines whole, scrolling sideways,
/// with the far edge faded while more of a line lies past it.
class LogDetails extends StatefulWidget {
  const LogDetails({super.key, required this.text, this.initiallyOpen = false});

  final String text;
  final bool initiallyOpen;

  /// The fade over the far edge, while there is more to scroll to.
  static const fadeKey = ValueKey('log-details-fade');

  @override
  State<LogDetails> createState() => _LogDetailsState();
}

class _LogDetailsState extends State<LogDetails> {
  late bool _open = widget.initiallyOpen;

  /// Whether a line runs past the panel's far edge.
  var _more = false;

  bool _onMetrics(ScrollMetrics m) {
    final more = m.extentAfter > 0.5;
    if (more != _more) setState(() => _more = more);
    return false;
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    final toggle = Semantics(
      expanded: _open,
      child: ClaxButton(
        key: const ValueKey('log-details'),
        label: 'Details',
        icon: _open ? Icons.expand_less_rounded : Icons.expand_more_rounded,
        onPressed: () => setState(() => _open = !_open),
      ),
    );
    if (!_open) return toggle;
    Widget lines = NotificationListener<ScrollMetricsNotification>(
      onNotification: (n) => _onMetrics(n.metrics),
      child: NotificationListener<ScrollNotification>(
        onNotification: (n) => _onMetrics(n.metrics),
        child: SingleChildScrollView(
          scrollDirection: Axis.horizontal,
          padding: const EdgeInsets.all(ClaxSpace.s12),
          child: Text(
            widget.text,
            softWrap: false,
            style: ClaxType.mono(theme.platform).copyWith(color: p.fg),
          ),
        ),
      ),
    );
    // The fade is laid over the lines, so the scroll view under it stays
    // put as the fade comes and goes.
    lines = Stack(
      children: [
        lines,
        if (_more)
          PositionedDirectional(
            key: LogDetails.fadeKey,
            top: 0,
            bottom: 0,
            end: 0,
            width: 40,
            child: IgnorePointer(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  gradient: LinearGradient(
                    begin: AlignmentDirectional.centerStart,
                    end: AlignmentDirectional.centerEnd,
                    colors: [p.card.withValues(alpha: 0), p.card],
                  ),
                ),
              ),
            ),
          ),
      ],
    );
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        toggle,
        const SizedBox(height: ClaxSpace.s8),
        Container(
          key: const ValueKey('log-details-panel'),
          width: double.infinity,
          decoration: BoxDecoration(
            color: p.card,
            borderRadius: BorderRadius.circular(ClaxRadius.md),
            border: Border.all(color: p.border),
          ),
          clipBehavior: Clip.antiAlias,
          child: lines,
        ),
      ],
    );
  }
}
