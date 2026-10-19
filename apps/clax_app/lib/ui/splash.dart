import 'package:flutter/material.dart';

import '../daemon/clock.dart';
import 'kit/mark.dart';
import 'theme.dart';
import 'tokens.g.dart';

/// What the app shows while the service starts: the Clax mark, 132 wide at
/// the window's centre, where Android's splash screen draws it, so the
/// first frame takes over from the system's splash without a jump.
///
/// The mark rests. Once [breatheAfter] has passed on [clock] it breathes
/// ([ClaxMarkState.working]) and "Starting…" fades in under it, in
/// `callout` and `muted`; the mark does not move to make room.
class ClaxSplash extends StatefulWidget {
  const ClaxSplash({
    super.key,
    this.clock = const SystemClock(),
    this.breatheAfter = const Duration(milliseconds: 600),
  });

  final Clock clock;
  final Duration breatheAfter;

  /// The mark's width: Android's splash icon, 132 dp on its 288 dp canvas.
  static const double markSize = 132;

  @override
  State<ClaxSplash> createState() => _ClaxSplashState();
}

class _ClaxSplashState extends State<ClaxSplash>
    with SingleTickerProviderStateMixin {
  late final AnimationController _fade = AnimationController(
    vsync: this,
    duration: ClaxMotion.enter,
  );
  bool _breathing = false;

  @override
  void initState() {
    super.initState();
    widget.clock.delay(widget.breatheAfter).then((_) {
      if (!mounted) return;
      setState(() => _breathing = true);
      if (MediaQuery.disableAnimationsOf(context)) {
        _fade.value = 1;
      } else {
        _fade.forward();
      }
    });
  }

  @override
  void dispose() {
    _fade.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final p = claxPalette(context);
    const markHeight =
        ClaxSplash.markSize * ClaxMarkGeometry.height / ClaxMarkGeometry.width;
    return Scaffold(
      backgroundColor: p.bg,
      body: LayoutBuilder(
        builder: (context, box) => Stack(
          children: [
            Center(
              child: ClaxMark(
                size: ClaxSplash.markSize,
                state: _breathing
                    ? ClaxMarkState.working
                    : ClaxMarkState.resting,
              ),
            ),
            if (_breathing)
              Positioned(
                left: ClaxSpace.s24,
                right: ClaxSpace.s24,
                top: box.maxHeight / 2 + markHeight / 2 + ClaxSpace.s24,
                child: FadeTransition(
                  opacity: _fade,
                  child: Text(
                    'Starting…',
                    textAlign: TextAlign.center,
                    style: ClaxType.of(
                      ClaxRole.callout,
                      Theme.of(context).platform,
                    ).copyWith(color: p.muted),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}
