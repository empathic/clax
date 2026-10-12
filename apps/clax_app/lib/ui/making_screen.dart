import 'dart:async';

import 'package:flutter/material.dart';

import '../agent/backend.dart';
import '../agent/edit_result.dart';
import '../agent/request.dart';

/// Makes a new app from [request] and returns its artifact ID.
///
/// [cancelled] completes when the person leaves the making screen (Cancel,
/// back, Edit, Open Settings) or retries; pass it on as
/// `AgentLoop.newApp(request, cancelled: …)` so a run nobody waits for
/// creates nothing. [stage] reports what the run is doing (see
/// [StagedBackend]). Throws `AgentError`; an `AgentError.cancelled` after
/// [cancelled] fired is not shown.
typedef MakeApp = Future<String> Function(
  String request,
  Future<void> cancelled,
  void Function(MakeStage stage) stage,
);

/// What a making run is doing, as the making screen says it.
enum MakeStage {
  /// Before the model is asked: reading the other apps, building the prompt.
  starting('Starting the agent'),

  /// The model is writing the app.
  writing('Writing your app'),

  /// The first result was refused (it failed validation or an edit missed);
  /// the model is asked again with the reason.
  retrying('Fixing a problem and trying again'),

  /// The result came back; it is checked and published.
  publishing('Checking and publishing');

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

/// Runs [make] for [request] while showing [makingTitle], the run's
/// [MakeStage], the seconds elapsed (with "Usually under a minute" after
/// [slowAfter]) and Cancel. Pops [Made] with the new artifact's ID.
///
/// A failure stays on the screen with its reason, the request, Retry and
/// Edit, and a link to Settings when the reason names them. Each attempt
/// gets its own cancel signal, completed when the attempt is abandoned:
/// Cancel, back, Edit, Open Settings, a Retry, or the screen going away.
class MakingScreen extends StatefulWidget {
  const MakingScreen({
    super.key,
    required this.request,
    required this.make,
    this.openSettings,
  });

  final String request;
  final MakeApp make;
  final VoidCallback? openSettings;

  /// When the "Usually under a minute" hint appears.
  static const slowAfter = 20;

  @override
  State<MakingScreen> createState() => _MakingScreenState();
}

class _MakingScreenState extends State<MakingScreen> {
  Timer? _tick;
  int _seconds = 0;
  String? _failure;
  MakeStage _stage = MakeStage.starting;

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
    super.dispose();
  }

  /// Completes the running attempt's cancel signal.
  void _abandon() {
    final c = _cancel;
    _cancel = null;
    if (c != null && !c.isCompleted) c.complete();
  }

  Future<void> _run() async {
    _abandon();
    final cancel = Completer<void>();
    _cancel = cancel;
    setState(() {
      _failure = null;
      _seconds = 0;
      _stage = MakeStage.starting;
    });
    _tick?.cancel();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) {
      if (mounted) setState(() => _seconds++);
    });
    bool current() => mounted && identical(_cancel, cancel);
    try {
      final aid = await widget.make(widget.request, cancel.future, (s) {
        if (current()) setState(() => _stage = s);
      });
      if (!mounted || !identical(_cancel, cancel)) return;
      _tick?.cancel();
      _cancel = null;
      Navigator.of(context).pop(Made(aid));
    } catch (e) {
      if (!current()) return;
      _tick?.cancel();
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
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: AnimatedSwitcher(
                  duration: const Duration(milliseconds: 200),
                  child: _failure == null ? _making(context) : _failed(context),
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
    return Column(
      key: const ValueKey('making'),
      mainAxisSize: MainAxisSize.min,
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Icon(Icons.auto_awesome_rounded, size: 40, color: scheme.primary),
        const SizedBox(height: 24),
        Text(
          makingTitle(widget.request),
          textAlign: TextAlign.center,
          style: theme.textTheme.headlineSmall,
        ),
        const SizedBox(height: 24),
        ClipRRect(
          borderRadius: BorderRadius.circular(4),
          child: const LinearProgressIndicator(minHeight: 6),
        ),
        const SizedBox(height: 16),
        AnimatedSwitcher(
          duration: const Duration(milliseconds: 200),
          child: Text(
            _stage.label,
            key: ValueKey(_stage),
            textAlign: TextAlign.center,
            style: theme.textTheme.titleMedium,
          ),
        ),
        const SizedBox(height: 4),
        Text(
          '$_seconds s',
          textAlign: TextAlign.center,
          style: theme.textTheme.bodyMedium?.copyWith(
            color: scheme.onSurfaceVariant,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
        ),
        SizedBox(
          height: 24,
          child: _seconds >= MakingScreen.slowAfter
              ? Text(
                  'Usually under a minute',
                  textAlign: TextAlign.center,
                  style: theme.textTheme.bodyMedium?.copyWith(
                    color: scheme.onSurfaceVariant,
                  ),
                )
              : null,
        ),
        const SizedBox(height: 16),
        Center(
          child: OutlinedButton(onPressed: _leave, child: const Text('Cancel')),
        ),
      ],
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
