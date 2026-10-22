import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';

import 'kit/kit.dart';
import 'theme.dart';
import 'tokens.g.dart';

/// The lines the cover turns through after its first, every
/// [UpdatingCover.lineEvery].
const updatingLines = [
  'Firing up the neurons…',
  'Teaching it new tricks…',
  'Tightening the bolts…',
  'Polishing the pixels…',
  'Rearranging the furniture…',
  'Fluffing the cushions…',
  'Dusting off the corners…',
  'Putting everything back where it goes…',
];

/// An app's screen while an update runs on it (bundles spec §8.4): while
/// [updating], no page, but a [ClaxSynapse] over a line that names the
/// app ([title]: "Updating Notes…", or "Updating…" without one) and then
/// turns through [lines], shuffled for each showing, never the
/// same line twice in a row, cross-faded every [lineEvery].
///
/// When [updating] turns false the cover plays the synapse's finish
/// (updating → finishing → page) and only then builds [page]; under
/// reduced motion the mark is still, the lines only cross-fade, and the
/// page comes at once. A screen reader hears "Updating <title>" (or
/// "Updating"), not the turning lines.
class UpdatingCover extends StatefulWidget {
  const UpdatingCover({
    super.key,
    required this.updating,
    required this.page,
    this.title,
    this.lines = updatingLines,
    this.random,
    this.seed,
  });

  final bool updating;

  /// The app's page, built once the cover is gone.
  final WidgetBuilder page;

  /// The app's title; null or empty while unknown.
  final String? title;

  final List<String> lines;

  /// Shuffles [lines]; null for a fresh source.
  final math.Random? random;

  /// Seeds the synapse's rhythm.
  final int? seed;

  /// How long each line shows.
  static const lineEvery = Duration(seconds: 3);

  @override
  State<UpdatingCover> createState() => _UpdatingCoverState();
}

/// Where the cover is: over the update, playing its finish, or gone.
enum _Phase { updating, finishing, page }

class _UpdatingCoverState extends State<UpdatingCover> {
  late _Phase _phase = widget.updating ? _Phase.updating : _Phase.page;
  late final math.Random _random = widget.random ?? math.Random();
  Timer? _timer;

  /// The lines still to show this round, next last.
  final List<String> _deck = [];

  /// The line on screen; null for the first, which names the app.
  String? _line;

  @override
  void initState() {
    super.initState();
    if (_phase == _Phase.updating) _startLines();
  }

  void _startLines() {
    _line = null;
    _deck.clear();
    _timer?.cancel();
    if (widget.lines.isEmpty) return;
    _timer = Timer.periodic(UpdatingCover.lineEvery, (_) {
      setState(() => _line = _draw());
    });
  }

  /// The next line: the deck's top, dealing a new shuffled round when it
  /// runs out whose first line is not the one on screen.
  String _draw() {
    if (_deck.isEmpty) {
      _deck.addAll(widget.lines);
      _deck.shuffle(_random);
      if (_deck.length > 1 && _deck.last == _line) {
        final swap = _random.nextInt(_deck.length - 1);
        _deck[_deck.length - 1] = _deck[swap];
        _deck[swap] = _line!;
      }
    }
    return _deck.removeLast();
  }

  @override
  void didUpdateWidget(UpdatingCover old) {
    super.didUpdateWidget(old);
    if (widget.updating == old.updating) return;
    if (widget.updating) {
      if (_phase == _Phase.page) _startLines();
      _phase = _Phase.updating;
    } else {
      _phase = MediaQuery.disableAnimationsOf(context)
          ? _Phase.page
          : _Phase.finishing;
      if (_phase == _Phase.page) _timer?.cancel();
    }
  }

  void _finished() {
    if (!mounted || _phase != _Phase.finishing) return;
    _timer?.cancel();
    setState(() => _phase = _Phase.page);
  }

  @override
  void dispose() {
    _timer?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    if (_phase == _Phase.page) return widget.page(context);
    final p = claxPalette(context);
    final platform = Theme.of(context).platform;
    final title = widget.title;
    final named = title != null && title.isNotEmpty;
    final text = _line ?? (named ? 'Updating $title…' : 'Updating…');
    final reduced = MediaQuery.disableAnimationsOf(context);
    return Scaffold(
      body: Center(
        child: Semantics(
          container: true,
          liveRegion: false,
          label: named ? 'Updating $title' : 'Updating',
          child: ExcludeSemantics(
            child: Padding(
              padding: const EdgeInsets.symmetric(horizontal: ClaxSpace.s24),
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  ClaxSynapse(
                    size: 88,
                    seed: widget.seed,
                    finishing: _phase == _Phase.finishing,
                    onFinished: _finished,
                  ),
                  const SizedBox(height: ClaxSpace.s24),
                  AnimatedSwitcher(
                    duration: ClaxMotion.of(context, ClaxMotion.enter),
                    switchInCurve: ClaxMotion.decelerate,
                    switchOutCurve: ClaxMotion.accelerate,
                    transitionBuilder: (child, animation) => FadeTransition(
                      opacity: animation,
                      child: reduced
                          ? child
                          : SlideTransition(
                              position: Tween(
                                begin: const Offset(0, 0.35),
                                end: Offset.zero,
                              ).animate(animation),
                              child: child,
                            ),
                    ),
                    layoutBuilder: (current, previous) => Stack(
                      alignment: Alignment.center,
                      children: [...previous, ?current],
                    ),
                    child: Text(
                      text,
                      key: ValueKey(text),
                      textAlign: TextAlign.center,
                      style: ClaxType.of(
                        ClaxRole.callout,
                        platform,
                      ).copyWith(color: p.muted),
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
