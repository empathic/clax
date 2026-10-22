import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'host_protocol.dart' show ChromeMsg;
import 'kit/scaffold.dart' show statusStyleFor;
import 'theme.dart';

/// The system bars over an app's page, drawn edge to edge: [child] takes
/// the whole height it is given, and the status-bar band (the top inset)
/// is painted in [top], the navigation-bar band (the bottom inset) in
/// [bottom], over it. Each band carries the bar style [statusStyleFor] picks
/// for its colour, so the system's icons are light over a dark band and
/// dark over a light one.
///
/// [top] and [bottom] are the page's own edge colours, from the shell's
/// `chrome` message ([ChromeMsg]); null, before the page has said, is the
/// palette's `bg`. A band with no inset under it (no bar there, or the
/// bottom inset removed because something else holds it) is not drawn.
class SystemBands extends StatelessWidget {
  const SystemBands({
    super.key,
    required this.top,
    required this.bottom,
    required this.child,
  });

  final Color? top;
  final Color? bottom;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    final bg = claxPalette(context).bg;
    final top = this.top ?? bg;
    final bottom = this.bottom ?? bg;
    final pad = MediaQuery.paddingOf(context);
    Widget band(String key, Color color, double height) =>
        AnnotatedRegion<SystemUiOverlayStyle>(
          value: statusStyleFor(color),
          child: IgnorePointer(
            child: ColoredBox(
              key: ValueKey('system-band-$key'),
              color: color,
              child: SizedBox(height: height, width: double.infinity),
            ),
          ),
        );
    return Stack(
      fit: StackFit.expand,
      children: [
        child,
        if (pad.top > 0)
          Positioned(left: 0, right: 0, top: 0, child: band('top', top, pad.top)),
        if (pad.bottom > 0)
          Positioned(
            left: 0,
            right: 0,
            bottom: 0,
            child: band('bottom', bottom, pad.bottom),
          ),
      ],
    );
  }
}
