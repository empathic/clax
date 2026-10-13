import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../agent/backend.dart';
import '../agent/edit_result.dart';
import '../agent/progress.dart';
import '../agent/request.dart';
import '../agent/timings.dart';
import '../daemon/clock.dart';
import 'making_preview.dart';

/// Makes a new app from [request] and returns its artifact ID.
///
/// [cancelled] completes when the person leaves the making screen (Cancel,
/// back, Edit, Open Settings) or retries; pass it on as
/// `AgentLoop.newApp(request, cancelled: …)` so a run nobody waits for
/// creates nothing. [report] takes what the run is doing: its [MakeStage]s
/// (see [StagedBackend]) and, wrapped in [MakeProgress], each event of
/// `AgentLoop.newApp`'s `onProgress`, which the screen shows as the live
/// preview and narration. Throws `AgentError`; an `AgentError.cancelled`
/// after [cancelled] fired is not shown.
typedef MakeApp = Future<String> Function(
  String request,
  Future<void> cancelled,
  void Function(MakeUpdate update) report,
);

/// What a making run reports to the making screen: a [MakeStage] or a
/// [MakeProgress].
sealed class MakeUpdate {}

/// One progress event of the run (narration, a tool call, the page as it
/// is written, a render's screenshot).
final class MakeProgress implements MakeUpdate {
  const MakeProgress(this.event);

  final AgentProgress event;

  @override
  String toString() => 'MakeProgress($event)';
}

/// What a making run is doing, as the making screen says it.
enum MakeStage implements MakeUpdate {
  /// Before the model is asked: reading the other apps, building the prompt.
  starting('Starting the agent'),

  /// The model is writing the app.
  writing('Writing your app'),

  /// The first result was refused (it failed validation or an edit missed);
  /// the model is asked again with the reason.
  retrying('Fixing a problem and trying again'),

  /// The result came back; it is checked and published.
  publishing('Checking and publishing'),

  /// The device cut Clax's network while it was in the background; the
  /// run waits for Clax to come back to the front.
  paused('Waiting for Clax to come back…'),

  /// Clax is back after a cut; the model is asked again.
  resuming('Picking up where it left off');

  const MakeStage(this.label);

  final String label;
}

/// An [AgentBackend] that reports [MakeStage]s around another backend's
/// runs: [MakeStage.writing] when the first run starts,
/// [MakeStage.retrying] when a later one does, and [MakeStage.publishing]
/// when a run returns a result. Wrap the backend of one new-app run with it.
class StagedBackend implements AgentBackend {
  StagedBackend(this.inner, this.stage);

  final AgentBackend inner;
  final void Function(MakeStage stage) stage;
  int _runs = 0;

  @override
  String get name => inner.name;

  @override
  Future<EditResult> run(AgentRequest request) async {
    stage(_runs++ == 0 ? MakeStage.writing : MakeStage.retrying);
    final r = await inner.run(request);
    stage(MakeStage.publishing);
    return r;
  }
}

/// How the making screen ended, when it did not end in Cancel (which pops
/// null).
sealed class MakingResult {
  const MakingResult();
}

/// The app was made; [aid] is its artifact ID.
class Made extends MakingResult {
  const Made(this.aid);
  final String aid;
}

/// The person chose to change the request after a failure.
class EditRequest extends MakingResult {
  const EditRequest();
}

/// The longest part of a request the making title quotes.
const makingTitleMax = 48;

const _joiners = {
  'a', 'an', 'and', 'as', 'at', 'by', 'for', 'from', 'in', 'into', 'my', //
  'of', 'on', 'or', 'that', 'the', 'to', 'with', 'your',
};

/// The making screen's title: "Making", the request in quotes, and "…".
/// A request longer than [makingTitleMax] characters is cut at its last
/// word boundary within them, short joining words ("and", "a", …) at the
/// cut are dropped, and the "…" moves inside the quotes.
String makingTitle(String request) {
  final r = request.trim().replaceAll(RegExp(r'\s+'), ' ');
  if (r.length <= makingTitleMax) return 'Making “$r”…';
  var cut = r.lastIndexOf(' ', makingTitleMax);
  if (cut <= 0) cut = makingTitleMax;
  final words = r.substring(0, cut).trimRight().split(' ');
  // "…with a weekly total and a…" reads better as "…weekly total…".
  while (words.length > 1 && _joiners.contains(words.last.toLowerCase())) {
    words.removeLast();
  }
  return 'Making “${words.join(' ')}…”';
}

/// The say→first-output timing: from Make to the first frame of the new
/// app's streamed `index.html` in the making screen's preview.
const sayFirstOutput = 'say-first-output';

/// What the making screen shows of one attempt, built from its
/// [MakeUpdate]s:
///
/// - [line]: the latest step (a [Narration] or a [ToolStarted] label), or
///   the [stage] before any, or while the run waits out a network cut;
/// - [earlier]: up to two steps before it, oldest first;
/// - [html]: `index.html` as written so far, for the live preview;
/// - [png] and [errors]: the last render's screenshot and its uncaught
///   errors, shown while [inspected] (until `index.html` is written again);
/// - [asking]: the agent waits on a question to the person.
///
/// The preview never shrinks within a step: text shorter than [html] (a
/// rewrite of the page, or a resend after [AttemptStarted], starting over)
/// is held, and the screenshot with it, until the text is as long again or
/// the next step begins, so neither blanks what is shown.
class MakingLog {
  MakeStage stage = MakeStage.starting;
  final List<String> _steps = [];
  String? html;
  Uint8List? png;
  int errors = 0;
  bool inspected = false;
  bool asking = false;

  /// `index.html` text shorter than [html], held until the next step.
  String? _held;

  String get line => switch (stage) {
    MakeStage.paused || MakeStage.resuming => stage.label,
    _ => _steps.isEmpty ? stage.label : _steps.last,
  };

  List<String> get earlier =>
      _steps.isEmpty ? const [] : _steps.sublist(0, _steps.length - 1);

  /// Takes [u]; returns whether [html] changed.
  bool apply(MakeUpdate u) {
    switch (u) {
      case final MakeStage s:
        stage = s;
        return false;
      case MakeProgress(:final event):
        return _progress(event);
    }
  }

  bool _progress(AgentProgress e) {
    switch (e) {
      case AttemptStarted():
        asking = false;
        return false;
      case Narration(:final text):
        _step(text);
        asking = false;
        return _release();
      case ToolStarted(:final name, :final label):
        _step(label);
        asking = name == 'ask';
        if (stage == MakeStage.starting) stage = MakeStage.writing;
        return _release();
      case PartialFile(:final path, :final text) when path == 'index.html':
        asking = false;
        if (text.length < (html?.length ?? 0)) {
          _held = text;
          return false;
        }
        _held = null;
        html = text;
        inspected = false;
        return true;
      case Inspected(png: final shot, errors: final n):
        png = shot;
        errors = n;
        inspected = true;
        asking = false;
        return false;
      case PartialFile() || PartialEdit() || OutputTokens():
        return false;
    }
  }

  /// Ends a hold: the held text shows, even though shorter.
  bool _release() {
    final held = _held;
    if (held == null) return false;
    _held = null;
    html = held;
    inspected = false;
    return true;
  }

  void _step(String text) {
    final t = text.trim();
    if (t.isEmpty || (_steps.isNotEmpty && _steps.last == t)) return;
    _steps.add(t);
    if (_steps.length > 3) _steps.removeAt(0);
  }
}

/// Runs [make] for [request] while showing [makingTitle], what the run is
/// doing ([MakingLog.line] and the steps before it), the seconds elapsed
/// (with "Usually under a minute" after [slowAfter]) and Cancel. Pops
/// [Made] with the new artifact's ID, or, with [onMade], hands the new app
/// to it instead (which replaces this route, completing it with [Made]).
///
/// - **Preview.** While `index.html` is written, a [PreviewView] from
///   [preview] shows it, loaded at most four times a second
///   ([PreviewThrottle] on [clock]); after a render, its screenshot shows
///   instead, with the number of errors it found. The first preview load
///   of an attempt stops [sayFirstOutput] on [timings], which each attempt
///   starts. Without a preview (no WebView on the platform) the card keeps
///   its placeholder.
/// - **Questions.** While the agent waits on a question, the elapsed
///   seconds stop.
/// - **Failure** stays on the screen with its reason, the request, Retry
///   and Edit, and a link to Settings when the reason names them. Each
///   attempt gets its own cancel signal, completed when the attempt is
///   abandoned: Cancel, back, Edit, Open Settings, a Retry, or the screen
///   going away.
class MakingScreen extends StatefulWidget {
  const MakingScreen({
    super.key,
    required this.request,
    required this.make,
    this.openSettings,
    this.preview,
    this.timings,
    this.onMade,
    this.clock = const SystemClock(),
  });

  final String request;
  final MakeApp make;
  final VoidCallback? openSettings;

  /// Makes the live preview's view; null uses a [WebViewPreview] where the
  /// platform has a WebView.
  final PreviewView Function()? preview;

  /// Where [sayFirstOutput] is recorded; null records nothing.
  final Timings? timings;

  /// Takes the new app [aid] in place of the pop, with the screen's
  /// context and the last render's screenshot ([MakingLog.png]; null when
  /// none), to put the app's route in this one's place
  /// (`pushReplacement(…, result: Made(aid))`).
  final void Function(BuildContext context, String aid, Uint8List? lastFrame)?
  onMade;

  /// Spaces the preview's loads.
  final Clock clock;

  /// When the "Usually under a minute" hint appears.
  static const slowAfter = 20;

  @override
  State<MakingScreen> createState() => _MakingScreenState();
}

class _MakingScreenState extends State<MakingScreen> {
  Timer? _tick;
  int _seconds = 0;
  String? _failure;
  var _log = MakingLog();

  PreviewView? _view;
  PreviewThrottle? _throttle;

  /// Whether the current attempt's preview has loaded a frame yet.
  var _shown = false;

  /// The running attempt's cancel signal; a stale attempt's result is
  /// dropped because its completer is no longer this one.
  Completer<void>? _cancel;

  @override
  void initState() {
    super.initState();
    _run();
  }

  @override
  void dispose() {
    _tick?.cancel();
    _abandon();
    _throttle?.close();
    _view?.dispose();
    super.dispose();
  }

  /// Completes the running attempt's cancel signal.
  void _abandon() {
    final c = _cancel;
    _cancel = null;
    if (c != null && !c.isCompleted) c.complete();
  }

  PreviewView? _makeView() {
    final make = widget.preview;
    if (make != null) return make();
    if (!WebViewPreview.available) return null;
    return WebViewPreview(background: Theme.of(context).colorScheme.surface);
  }

  /// Whether making the preview's view failed; the card then keeps its
  /// placeholder and the run goes on.
  var _noView = false;

  void _load(String html) {
    if (_noView) return;
    PreviewView? view;
    try {
      view = _view ??= _makeView();
    } on Object catch (_) {
      view = null;
    }
    if (view == null) {
      _noView = true;
      return;
    }
    unawaited(view.loadHtml(html).catchError((Object _) {}));
    if (!_shown) {
      _shown = true;
      widget.timings?.stop(sayFirstOutput, extra: {'chars': html.length});
      if (mounted) setState(() {});
    }
  }

  Future<void> _run() async {
    _abandon();
    final cancel = Completer<void>();
    _cancel = cancel;
    _throttle?.close();
    final throttle = _throttle = PreviewThrottle(
      load: _load,
      clock: widget.clock,
    );
    widget.timings?.start(sayFirstOutput);
    setState(() {
      _failure = null;
      _seconds = 0;
      _log = MakingLog();
      _shown = false;
    });
    _tick?.cancel();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted && !_log.asking) setState(() => _seconds++);
    });
    bool current() => mounted && identical(_cancel, cancel);
    void report(MakeUpdate u) {
      if (!current()) return;
      final htmlChanged = _log.apply(u);
      if (htmlChanged) {
        throttle.offer(_log.html!);
      } else if (u is MakeProgress &&
          (u.event is ToolStarted || u.event is Inspected)) {
        // A write ended: its last text shows now, not a beat later.
        throttle.flush();
      }
      setState(() {});
    }

    try {
      final aid = await widget.make(widget.request, cancel.future, report);
      if (!mounted || !identical(_cancel, cancel)) return;
      _tick?.cancel();
      _cancel = null;
      if (widget.onMade case final onMade?) {
        onMade(context, aid, _log.png);
      } else {
        Navigator.of(context).pop(Made(aid));
      }
    } catch (e) {
      if (!current()) return;
      _tick?.cancel();
      throttle.clear();
      setState(() => _failure = e is AgentError ? e.message : '$e');
    }
  }

  void _leave([MakingResult? result]) {
    _abandon();
    Navigator.of(context).pop(result);
  }

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: true,
      onPopInvokedWithResult: (didPop, _) {
        if (didPop) _abandon();
      },
      child: Scaffold(
        body: SafeArea(
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 480),
              child: AnimatedSwitcher(
                duration: const Duration(milliseconds: 200),
                child: _failure == null
                    ? _making(context)
                    // Scrolls, so its buttons stay reachable with large
                    // text on a small screen.
                    : SingleChildScrollView(
                        key: const ValueKey('failed'),
                        padding: const EdgeInsets.all(24),
                        child: _failed(context),
                      ),
              ),
            ),
          ),
        ),
      ),
    );
  }

  Widget _making(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final log = _log;
    final dim = theme.textTheme.bodyMedium?.copyWith(
      color: scheme.onSurfaceVariant,
    );
    // With large text the card takes half the height and
    // the whole view scrolls, so Cancel stays reachable.
    Widget layout(BoxConstraints c, List<Widget> Function(Widget card) kids) {
      const pad = EdgeInsets.fromLTRB(16, 12, 16, 16);
      final scaled = MediaQuery.textScalerOf(context).scale(16) > 20;
      if (!scaled) {
        return Padding(
          padding: pad,
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: kids(Expanded(child: _card(context))),
          ),
        );
      }
      return SingleChildScrollView(
        padding: pad,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: kids(
            SizedBox(height: c.maxHeight * 0.5, child: _card(context)),
          ),
        ),
      );
    }

    return LayoutBuilder(
      key: const ValueKey('making'),
      builder: (context, c) => layout(
        c,
        (card) => [
          Text(
            makingTitle(widget.request),
            textAlign: TextAlign.center,
            maxLines: 2,
            overflow: TextOverflow.ellipsis,
            style: theme.textTheme.titleLarge,
          ),
          const SizedBox(height: 12),
          card,
          const SizedBox(height: 12),
          ClipRRect(
            borderRadius: BorderRadius.circular(4),
            child: const LinearProgressIndicator(minHeight: 4),
          ),
          const SizedBox(height: 12),
          for (final step in log.earlier)
            Text(
              step,
              key: ValueKey('earlier:$step'),
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: dim?.copyWith(
                color: scheme.onSurfaceVariant.withValues(alpha: 0.7),
              ),
            ),
          Row(
            crossAxisAlignment: CrossAxisAlignment.baseline,
            textBaseline: TextBaseline.alphabetic,
            children: [
              Expanded(
                child: AnimatedSwitcher(
                  duration: const Duration(milliseconds: 200),
                  layoutBuilder: (current, previous) => Stack(
                    alignment: Alignment.centerLeft,
                    children: [...previous, ?current],
                  ),
                  child: Text(
                    log.line,
                    key: ValueKey(log.line),
                    maxLines: 2,
                    overflow: TextOverflow.ellipsis,
                    style: theme.textTheme.titleMedium,
                  ),
                ),
              ),
              const SizedBox(width: 12),
              Text(
                log.asking ? 'waiting for you' : '$_seconds s',
                style: dim?.copyWith(
                  fontFeatures: const [FontFeature.tabularFigures()],
                ),
              ),
            ],
          ),
          SizedBox(
            height: 24,
            child: _seconds >= MakingScreen.slowAfter
                ? Text('Usually under a minute', style: dim)
                : null,
          ),
          Center(
            child: OutlinedButton(
              onPressed: _leave,
              child: const Text('Cancel'),
            ),
          ),
        ],
      ),
    );
  }

  /// The preview card: a placeholder until the live preview has a frame;
  /// the render's screenshot over the preview after a render, until the
  /// page is written again. The live view stays mounted under the
  /// screenshot, so going back to it starts no new platform view.
  Widget _card(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final log = _log;
    final png = log.png;
    final view = _view;
    final live = _shown && view != null;
    final shot = log.inspected && png != null;
    final badge = switch ((shot, log.errors)) {
      (true, 0) => 'Checked',
      (true, 1) => '1 error to fix',
      (true, final n) => '$n errors to fix',
      _ when live => 'Live',
      _ => null,
    };
    const fade = Duration(milliseconds: 250);
    return DecoratedBox(
      decoration: BoxDecoration(
        color: scheme.surfaceContainerLow,
        borderRadius: BorderRadius.circular(20),
        border: Border.all(color: scheme.outlineVariant),
      ),
      child: ClipRRect(
        borderRadius: BorderRadius.circular(19),
        child: Stack(
          fit: StackFit.expand,
          children: [
            AnimatedOpacity(
              opacity: live ? 0 : 1,
              duration: fade,
              child: _placeholder(theme),
            ),
            if (live)
              KeyedSubtree(
                key: const ValueKey('preview'),
                child: view.build(context),
              ),
            if (png != null)
              IgnorePointer(
                child: AnimatedOpacity(
                  opacity: shot ? 1 : 0,
                  duration: fade,
                  child: ColoredBox(
                    color: scheme.surface,
                    child: Image.memory(
                      png,
                      key: const ValueKey('inspection'),
                      fit: BoxFit.fitWidth,
                      alignment: Alignment.topCenter,
                      gaplessPlayback: true,
                    ),
                  ),
                ),
              ),
            if (badge != null)
              Positioned(
                top: 8,
                right: 8,
                child: _Badge(text: badge, error: shot && log.errors > 0),
              ),
          ],
        ),
      ),
    );
  }

  Widget _placeholder(ThemeData theme) {
    final scheme = theme.colorScheme;
    return Center(
      key: const ValueKey('placeholder'),
      child: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.auto_awesome_rounded, size: 40, color: scheme.primary),
            const SizedBox(height: 16),
            Text(
              'Your app appears here as it is written',
              textAlign: TextAlign.center,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: scheme.onSurfaceVariant,
              ),
            ),
          ],
        ),
      ),
    );
  }

  Widget _failed(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final reason = _failure!;
    final settings = widget.openSettings != null && reason.contains('Settings');
    return Column(
      key: const ValueKey('failed'),
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Icon(Icons.error_outline_rounded, size: 40, color: scheme.error),
        const SizedBox(height: 16),
        Text(
          "Couldn't make it",
          textAlign: TextAlign.center,
          style: theme.textTheme.headlineSmall,
        ),
        const SizedBox(height: 8),
        Text(
          reason,
          textAlign: TextAlign.center,
          style: theme.textTheme.bodyLarge?.copyWith(
            color: scheme.onSurfaceVariant,
          ),
        ),
        const SizedBox(height: 24),
        DecoratedBox(
          decoration: BoxDecoration(
            color: scheme.surfaceContainerHighest,
            borderRadius: BorderRadius.circular(16),
          ),
          child: Padding(
            padding: const EdgeInsets.all(16),
            child: Text(widget.request, style: theme.textTheme.bodyLarge),
          ),
        ),
        const SizedBox(height: 24),
        Wrap(
          alignment: WrapAlignment.center,
          spacing: 12,
          runSpacing: 12,
          children: [
            if (settings)
              FilledButton(
                onPressed: () {
                  _leave();
                  widget.openSettings!();
                },
                child: const Text('Open Settings'),
              ),
            settings
                ? OutlinedButton(onPressed: _run, child: const Text('Retry'))
                : FilledButton(onPressed: _run, child: const Text('Retry')),
            TextButton(
              onPressed: () => _leave(const EditRequest()),
              child: const Text('Edit'),
            ),
            TextButton(onPressed: _leave, child: const Text('Cancel')),
          ],
        ),
      ],
    );
  }
}

/// A small label over the preview's corner: "Live", "Checked", or the
/// errors a render found.
class _Badge extends StatelessWidget {
  const _Badge({required this.text, required this.error});

  final String text;
  final bool error;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final scheme = theme.colorScheme;
    final bg = error ? scheme.errorContainer : scheme.secondaryContainer;
    final fg = error ? scheme.onErrorContainer : scheme.onSecondaryContainer;
    return DecoratedBox(
      decoration: BoxDecoration(
        color: bg,
        borderRadius: BorderRadius.circular(12),
      ),
      child: Padding(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
        child: Text(
          text,
          style: theme.textTheme.labelMedium?.copyWith(color: fg),
        ),
      ),
    );
  }
}
