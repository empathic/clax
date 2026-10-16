import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';

import '../agent/backend.dart';
import '../agent/edit_result.dart';
import '../agent/progress.dart';
import '../agent/request.dart';
import '../agent/timings.dart';
import '../agent/plan.dart';
import '../agent/worker.dart';
import '../daemon/clock.dart';
import 'build_plan.dart';
import 'make_service.dart';
import 'making_preview.dart';

/// Makes a new app from [request] and returns its artifact ID.
///
/// [cancelled] completes when the person stops the make (Cancel, Edit,
/// Open Settings) or retries; pass it on as
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

/// The key the run keeps its plan under until the app exists
/// (`AgentLoop.newApp(planKey: …)`), for the build view to show the plan
/// from the first seconds.
final class MakePlanKey implements MakeUpdate {
  const MakePlanKey(this.key);

  final String key;

  @override
  String toString() => 'MakePlanKey($key)';
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

  /// The Claude API cannot be reached; the run waits for the connection
  /// and goes on by itself once it is back.
  offline('Waiting for connection'),

  /// Clax or the connection is back after a cut; the model is asked
  /// again.
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

/// The person went back to the apps, leaving the make (or the build) going.
class MakingLeft extends MakingResult {
  const MakingLeft();
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
///   the [stage] before any, or while the run waits out a network cut or
///   for the connection;
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
    MakeStage.paused || MakeStage.offline || MakeStage.resuming => stage.label,
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
      case MakePlanKey():
        return false;
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
      case PartialFile() ||
          PartialEdit() ||
          OutputTokens() ||
          WaitingForConnection() ||
          ConnectionBack():
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

/// Shows a make ([session], or one of its own running [make] for
/// [request]) with [makingTitle], what the run is doing ([MakingLog.line]
/// and the steps before it), the seconds elapsed (with "Usually under a
/// minute" after [slowAfter]) and Cancel. Pops [Made] with the new
/// artifact's ID, or, with [onMade], hands the new app to it instead (which
/// replaces this route, completing it with [Made]).
///
/// - **Leaving.** The top bar's "Back to apps" (and the platform's back)
///   pops [MakingLeft] and leaves a [session]'s make running; attached
///   again later, the screen catches up from the session (the preview, the
///   plan, the narration, a failure). Cancel stops the make and pops null.
///   A make of the screen's own (no [session]) stops as the screen goes.
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
///   abandoned: Cancel, Edit, Open Settings or a Retry.
/// - **Build view.** With [build], the screen is the build's view (a
///   Retry starts from the plan the failed run left, the person's edits
///   included): once
///   the run reports its [MakePlanKey], the plan shows as a live checklist
///   in a sheet over the bottom ([BuildPlanPanel]), which the person edits
///   as it builds. When the first version lands and its plan still has
///   features to build, the screen stays: "Open app" hands the app over
///   ([onMade]) whenever the person wants, while the worker's continue
///   runs go on, their progress shown in the preview, the narration and
///   the checklist, with Stop and Resume. Without a plan to go on with, the
///   app is handed over at once, as without [build].
class MakingScreen extends StatefulWidget {
  const MakingScreen({
    super.key,
    required this.request,
    this.make,
    this.session,
    this.openSettings,
    this.preview,
    this.timings,
    this.onMade,
    this.build,
    this.clock = const SystemClock(),
  });

  final String request;

  /// Runs the screen's own make, when there is no [session].
  final MakeApp? make;

  /// The make to show, which goes on when the screen goes.
  final MakeSession? session;
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

  /// The plans and builds, for the build view; null shows none.
  final BuildLink? build;

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

  late final MakeSession _session;
  StreamSubscription<MakeUpdate>? _updates;
  MakingLog get _log => _session.log;

  PreviewView? _view;
  PreviewThrottle? _throttle;

  /// Whether the current attempt's preview has loaded a frame yet.
  var _shown = false;

  /// The key the plan is kept under: the run's provisional one, then the
  /// app's ID.
  String? _planKey;
  Plan? _plan;
  BuildState? _buildState;
  StreamSubscription<Plan?>? _planSub;
  StreamSubscription<BuildState>? _buildSub;

  /// The app, once its first version landed and its build goes on here.
  String? _made;

  /// The plan a failed run left, for the Retry's run to start from.
  Plan? _carry;

  /// Whether the session's made app was taken up here.
  var _tookMade = false;

  /// Whether the screen shows a make waiting for a slot.
  var _waiting = false;

  @override
  void initState() {
    super.initState();
    final own = widget.session == null;
    _session =
        widget.session ??
        MakeSession(
          widget.request,
          widget.make ?? (throw ArgumentError('a make or a session')),
          clock: widget.clock,
        );
    _session.attach();
    _session.addListener(_onSession);
    _updates = _session.updates.listen(_onUpdate);
    if (own || !_session.started && !_session.queued) {
      // After this build: starting tells the session's listeners (the
      // launcher's tiles), which must not rebuild during it.
      _attempt(
        () => scheduleMicrotask(() {
          if (!_session.cancelled && !_session.started && !_session.queued) {
            unawaited(_session.start());
          }
        }),
      );
    } else {
      _catchUp();
    }
  }

  @override
  void dispose() {
    _tick?.cancel();
    unawaited(_planSub?.cancel());
    unawaited(_buildSub?.cancel());
    unawaited(_updates?.cancel());
    _session.removeListener(_onSession);
    _session.detach();
    // A make of the screen's own has nobody else to show it.
    if (widget.session == null) _session.cancel();
    _throttle?.close();
    _view?.dispose();
    super.dispose();
  }

  /// Shows what [_session] has done so far: its page in the preview, its
  /// plan, its seconds, and how it ended.
  void _catchUp() {
    _fresh();
    _waiting = _session.queued;
    _seconds = _session.running
        ? widget.clock.now().difference(_session.startedAt).inSeconds
        : 0;
    if (_session.running && _log.html == null) {
      widget.timings?.start(sayFirstOutput);
    }
    if (_session.planKey case final key?) _watchPlan(key);
    if (_log.html case final html?) _throttle!.offer(html);
    if (_session.running) _ticking();
    _onSession();
  }

  /// A fresh preview throttle, for a new attempt or a catch-up.
  void _fresh() {
    _throttle?.close();
    _throttle = PreviewThrottle(load: _load, clock: widget.clock);
    _shown = false;
  }

  void _ticking() {
    _tick?.cancel();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted && !_log.asking) setState(() => _seconds++);
    });
  }

  /// One update of the running attempt, which the session's log took.
  void _onUpdate(MakeUpdate u) {
    if (!mounted) return;
    if (u is MakePlanKey) {
      _watchPlan(u.key);
      return;
    }
    final throttle = _throttle!;
    if (_session.lastChangedHtml) {
      throttle.offer(_log.html!);
    } else if (u is MakeProgress &&
        (u.event is ToolStarted || u.event is Inspected)) {
      // A write ended: its last text shows now, not a beat later.
      throttle.flush();
    }
    setState(() {});
  }

  /// The session made its app, failed or was cancelled.
  void _onSession() {
    if (!mounted) return;
    if (_waiting && _session.running) {
      // The service started the make this screen waited on.
      _waiting = false;
      _attempt(() {});
      return;
    }
    if (_session.aid case final aid? when !_tookMade) {
      _tookMade = true;
      unawaited(_onMade(aid));
    } else if (_session.failure case final f? when !_session.running) {
      if (_failure == f) return;
      _tick?.cancel();
      _throttle?.clear();
      setState(() => _failure = f);
    }
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

  /// Starts an attempt with [start]: a fresh preview, seconds and plan.
  void _attempt(void Function() start) {
    _fresh();
    widget.timings?.start(sayFirstOutput);
    unawaited(_planSub?.cancel());
    _planSub = null;
    // A Retry starts from the plan the failed run left, edits included.
    final carry = _plan;
    _carry = carry != null && carry.features.isNotEmpty ? carry : null;
    _failure = null;
    _seconds = 0;
    _planKey = null;
    _plan = null;
    _ticking();
    start();
    if (mounted) setState(() {});
  }

  /// Retry after a failure.
  void _run() => _attempt(() => unawaited(_session.start()));

  /// The made app [aid]: its build goes on here, or it is handed over.
  Future<void> _onMade(String aid) async {
    final throttle = _throttle!;
    if (await _goesOn(aid)) {
      if (mounted) _follow(aid, throttle);
      return;
    }
    _tick?.cancel();
    if (!mounted) return;
    if (widget.onMade case final onMade?) {
      onMade(context, aid, _log.png);
    } else {
      Navigator.of(context).pop(Made(aid));
    }
  }

  /// Watches the plan kept under [key], keeping the last one shown while
  /// the key has none (the moment the plan moves to the app's ID).
  void _watchPlan(String key) {
    final b = widget.build;
    if (b == null || key == _planKey) return;
    unawaited(_planSub?.cancel());
    setState(() => _planKey = key);
    if (_carry case final carry?) {
      _carry = null;
      // The run's first plan_set then finds this plan and is shown it.
      unawaited(
        b.plans
            .set(key, carry.features, request: carry.request)
            .catchError((Object _) => carry),
      );
    }
    _planSub = b.plans.watch(key).listen((p) {
      if (!mounted || p == null && _plan != null) return;
      setState(() => _plan = p);
    });
  }

  /// Whether app [aid]'s build goes on after its first version: its plan
  /// has features to build and is not stopped.
  Future<bool> _goesOn(String aid) async {
    final b = widget.build;
    if (b == null) return false;
    try {
      final p = await b.plans.get(aid);
      return p != null && !p.stopped && p.todo.isNotEmpty;
    } on Object {
      return false;
    }
  }

  /// Follows app [aid]'s build here: its plan, and each continue run's
  /// progress in the preview and the narration.
  void _follow(String aid, PreviewThrottle throttle) {
    final b = widget.build!;
    _watchPlan(aid);
    setState(() {
      _made = aid;
      _buildState = b.control.buildOf(aid);
    });
    _buildSub = b.control.builds.where((s) => s.appId == aid).listen((s) {
      if (!mounted) return;
      if (s.progress case final p?) {
        if (_log.apply(MakeProgress(p))) {
          throttle.offer(_log.html!);
        } else if (p is ToolStarted || p is Inspected) {
          throttle.flush();
        }
      }
      if (!buildGoing(s)) _tick?.cancel();
      setState(() => _buildState = s);
    });
  }

  /// Hands the made app over (Open app).
  void _openApp() {
    final aid = _made;
    if (aid == null) return;
    _tick?.cancel();
    if (widget.onMade case final onMade?) {
      onMade(context, aid, _log.png);
    } else {
      Navigator.of(context).pop(Made(aid));
    }
  }

  /// Stops the make and pops [result] (null: Cancel).
  void _leave([MakingResult? result]) {
    _session.cancel();
    Navigator.of(context).pop(result);
  }

  /// Back to the apps; the make or the build goes on.
  void _back() => Navigator.of(context).pop(const MakingLeft());

  @override
  Widget build(BuildContext context) {
    return PopScope(
      canPop: false,
      onPopInvokedWithResult: (didPop, _) {
        if (!didPop) _back();
      },
      child: Scaffold(
        body: SafeArea(
          child: Column(
            children: [
              _topBar(context),
              Expanded(
                child: Center(
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(maxWidth: 480),
                    child: AnimatedSwitcher(
                      duration: const Duration(milliseconds: 200),
                      child: _failure == null
                          ? _buildView(context)
                          // Scrolls, so its buttons stay reachable with
                          // large text on a small screen.
                          : SingleChildScrollView(
                              key: const ValueKey('failed'),
                              padding: const EdgeInsets.all(24),
                              child: _failed(context),
                            ),
                    ),
                  ),
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }

  /// "Back to apps", which leaves the make running.
  Widget _topBar(BuildContext context) => Padding(
    padding: const EdgeInsets.fromLTRB(4, 4, 16, 0),
    child: Align(
      alignment: Alignment.centerLeft,
      child: TextButton.icon(
        key: const ValueKey('build-back'),
        onPressed: _back,
        icon: const Icon(Icons.arrow_back_rounded),
        label: const Text('Back to apps'),
      ),
    ),
  );

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
                    _waiting ? 'Waiting for another make to finish' : log.line,
                    key: ValueKey(_waiting ? 'waiting' : log.line),
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
            child: _made == null && _seconds >= MakingScreen.slowAfter
                ? Text('Usually under a minute', style: dim)
                : null,
          ),
          Center(
            child: _made == null
                ? OutlinedButton(onPressed: _leave, child: const Text('Cancel'))
                : FilledButton.icon(
                    key: const ValueKey('open-app'),
                    onPressed: _openApp,
                    icon: const Icon(Icons.open_in_new_rounded, size: 18),
                    label: const Text('Open app'),
                  ),
          ),
        ],
      ),
    );
  }

  /// The making view, with the plan's sheet over its bottom once there is
  /// a plan to show ([MakingScreen.build] and a [MakePlanKey]). The making
  /// view keeps its place in the tree as the sheet comes, so the live
  /// preview is not built again.
  Widget _buildView(BuildContext context) {
    final b = widget.build;
    final key = _planKey;
    final sheet = b != null && key != null;
    final scale = MediaQuery.textScalerOf(context).scale(16) / 16;
    final peek = 104.0 + 36 * (scale - 1).clamp(0, 2);
    return LayoutBuilder(
      key: const ValueKey('making'),
      builder: (context, box) {
        final min = (peek / box.maxHeight).clamp(0.08, 0.5);
        return Stack(
          children: [
            Padding(
              padding: EdgeInsets.only(bottom: sheet ? peek : 0),
              child: _making(context),
            ),
            if (sheet)
              DraggableScrollableSheet(
                key: const ValueKey('plan-sheet'),
                initialChildSize: min,
                minChildSize: min,
                maxChildSize: 0.85,
                snap: true,
                builder: (context, controller) =>
                    _sheet(context, b, key, controller),
              ),
          ],
        );
      },
    );
  }

  Widget _sheet(
    BuildContext context,
    BuildLink b,
    String key,
    ScrollController controller,
  ) {
    final scheme = Theme.of(context).colorScheme;
    final made = _made;
    return Material(
      color: scheme.surfaceContainerLow,
      shape: RoundedRectangleBorder(
        side: BorderSide(color: scheme.outlineVariant),
        borderRadius: const BorderRadius.vertical(top: Radius.circular(24)),
      ),
      clipBehavior: Clip.antiAlias,
      child: BuildPlanPanel(
        plan: _plan,
        state: _buildState,
        plans: b.plans,
        planKey: key,
        controller: controller,
        onStop: made == null
            ? null
            : () => unawaited(b.control.stopBuild(made)),
        onResume: made == null
            ? null
            : () => unawaited(b.control.resumeBuild(made)),
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
                      errorBuilder: (_, _, _) => const SizedBox.shrink(),
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
    // Large text on a small phone may not fit the card: the hint is cut
    // rather than overflowing.
    return Center(
      key: const ValueKey('placeholder'),
      child: SingleChildScrollView(
        physics: const NeverScrollableScrollPhysics(),
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
