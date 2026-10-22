import 'dart:async';

import 'package:flutter/widgets.dart';

import '../platform/clock.dart';
import 'theme.dart';
import 'tokens.g.dart';
import 'working_chip.dart' show ClaxPill;

/// The strip that holds the Clax pill ([ClaxPill]) between the top inset
/// and an app's page, so the pill is never drawn over the page.
///
/// While [wanted] (the pill has something to say) the strip is open:
/// [PillStrip.heightOf] tall, on `bg`, directly under the top inset, with
/// the pill at its end, centred in it, and the page below it, given that
/// much less height. The strip keeps one height whether the pill shows its
/// words or is folded to the mark, so only opening and closing move the
/// page.
///
/// It opens as [wanted] turns true and closes once [wanted] has stayed
/// false for [grace] on [clock], so a pill that goes and comes back within
/// it keeps the strip open and the page still. Opening and closing slide
/// for [ClaxMotion.base] on [ClaxMotion.standard], and are instant with
/// reduced motion. An app opened while something already goes on starts
/// with the strip open.
///
/// The page is resized once per slide, not every frame: while the strip
/// slides the page keeps its whole height and moves (its bottom clipped),
/// and it is laid out the strip's height shorter only once the strip is
/// fully open, from which point closing gives the height back first.
///
/// [builder] builds the screen around the strip: `shown` is true while
/// the strip takes any height (opening, open or closing), for the status
/// bar's band to be `bg` with it; `slot` lays out a page, with a pill
/// (null for none) in the strip above it. The strip itself takes no taps
/// beyond the pill's own target, and passes none to the page.
class PillStrip extends StatefulWidget {
  const PillStrip({
    super.key,
    required this.wanted,
    required this.clock,
    required this.builder,
  });

  final bool wanted;
  final Clock clock;
  final Widget Function(
    BuildContext context,
    bool shown,
    Widget Function(Widget page, Widget? pill) slot,
  )
  builder;

  /// How tall the open strip is at 100% text: the pill's 48 dp target,
  /// the 32 dp pill 8 dp from each edge.
  static const height = ClaxLayout.target;

  /// How tall the open strip is at [context]'s text scale: [height], or
  /// the pill with its words ([ClaxPill.openHeight]) and 8 dp above and
  /// below it where large text makes it taller.
  static double heightOf(BuildContext context) {
    final open = ClaxPill.openHeight(context) + 2 * ClaxSpace.s8;
    return open > height ? open : height;
  }

  /// How long the strip stays open, empty, after the pill goes.
  static const grace = Duration(milliseconds: 600);

  @override
  State<PillStrip> createState() => _PillStripState();
}

class _PillStripState extends State<PillStrip>
    with SingleTickerProviderStateMixin {
  late final _slide = AnimationController(
    vsync: this,
    duration: ClaxMotion.base,
    value: widget.wanted ? 1 : 0,
  )..addStatusListener(_onStatus);
  late final _eased = CurvedAnimation(
    parent: _slide,
    curve: ClaxMotion.standard,
  );

  /// Whether the strip is open, or opening.
  late var _open = widget.wanted;

  /// The pending close's turn; a newer turn or [PillStrip.wanted] cancels it.
  var _close = 0;

  /// Whether motion is reduced, as of the last build.
  var _reduced = false;

  @override
  void didUpdateWidget(PillStrip old) {
    super.didUpdateWidget(old);
    if (widget.wanted) {
      _close++;
      if (!_open) {
        _open = true;
        _go(open: true);
      }
    } else if (old.wanted && _open) {
      final close = ++_close;
      unawaited(
        widget.clock.delay(PillStrip.grace).then((_) {
          if (!mounted || close != _close || widget.wanted) return;
          setState(() {
            _open = false;
            _go(open: false);
          });
        }),
      );
    }
  }

  /// Opens the strip, or closes it, at once with reduced motion. Its
  /// status says which way it goes: completed only fully open, dismissed
  /// only fully closed.
  void _go({required bool open}) {
    if (_reduced) {
      _slide.value = open ? 1 : 0;
    } else {
      unawaited(open ? _slide.forward() : _slide.reverse());
    }
  }

  void _onStatus(AnimationStatus status) {
    // Fully closed: the band goes back to the page's colour.
    if (status.isDismissed && mounted) setState(() {});
  }

  @override
  void dispose() {
    _eased.dispose();
    _slide.dispose();
    super.dispose();
  }

  Widget _slot(Widget page, Widget? pill) =>
      _StripSlot(slide: _slide, eased: _eased, page: page, pill: pill);

  @override
  Widget build(BuildContext context) {
    _reduced = MediaQuery.disableAnimationsOf(context);
    return widget.builder(context, _open || !_slide.isDismissed, _slot);
  }
}

/// The page under the strip, the strip's `bg` and the pill at its end, as
/// far open as [eased] says.
class _StripSlot extends StatelessWidget {
  const _StripSlot({
    required this.slide,
    required this.eased,
    required this.page,
    required this.pill,
  });

  final AnimationController slide;
  final Animation<double> eased;
  final Widget page;
  final Widget? pill;

  @override
  Widget build(BuildContext context) {
    final pad = MediaQuery.paddingOf(context);
    final bg = claxPalette(context).bg;
    final pill = this.pill;
    final height = PillStrip.heightOf(context);
    return AnimatedBuilder(
      animation: eased,
      builder: (context, _) {
        final s = height * eased.value;
        // Shorter only once fully open: while it slides the page keeps
        // its height and its bottom goes under the clip.
        final full = slide.isCompleted;
        return Stack(
          fit: StackFit.expand,
          children: [
            Positioned(
              left: 0,
              right: 0,
              top: s,
              bottom: full ? 0 : -s,
              child: page,
            ),
            Positioned(
              left: 0,
              right: 0,
              top: pad.top,
              height: s,
              child: ColoredBox(
                key: const ValueKey('pill-strip'),
                color: bg,
              ),
            ),
            if (pill != null)
              // The pill, centred in the strip's height, rides its lower
              // edge, coming out from under the status bar's band.
              Positioned(
                top: pad.top + s - height,
                height: height,
                left: pad.left + ClaxSpace.s16,
                right: pad.right + ClaxSpace.s16,
                child: Align(
                  alignment: AlignmentDirectional.centerEnd,
                  child: pill,
                ),
              ),
          ],
        );
      },
    );
  }
}
