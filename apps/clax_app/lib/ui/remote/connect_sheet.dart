/// Connecting the app to a hosted Clax by device sign-in (spec
/// 2026-10-21-app-push-pull §5.2): the sheet that shows the code while the
/// person approves this device in their browser.
library;

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart' show Clipboard, ClipboardData;

import '../../clax/api.dart' show ClaxApiError;
import '../../platform/clock.dart';
import '../../platform/external_links.dart';
import '../../remote/link_remote.dart' show originOf;
import '../../remote/models.dart';
import '../../remote/remote_api.dart';
import '../../remote/remote_errors.dart';
import '../bundle_sheets.dart';
import '../kit/kit.dart';
import '../theme.dart';

/// How long a sign-in's code lasts: the remote's device codes last 600 s.
const connectCodeLifetime = Duration(minutes: 10);

/// The longest machine name a remote takes, in characters.
const maxMachineName = 64;

/// "Clax on [device]" as a remote takes it: control characters dropped,
/// spaces trimmed, and cut to [maxMachineName] characters with "…" when
/// longer.
String machineName(String device) {
  final clean = String.fromCharCodes(
    device.runes.where((c) => !(c < 0x20 || (c >= 0x7f && c <= 0x9f))),
  ).trim();
  final full = 'Clax on ${clean.isEmpty ? 'this device' : clean}';
  final runes = full.runes.toList();
  if (runes.length <= maxMachineName) return full;
  return '${String.fromCharCodes(runes.take(maxMachineName - 1)).trimRight()}…';
}

/// Signs this device in to the hosted Clax at [url] through the core
/// ([RemoteApi.addRemote] as a `clax-app` client named "Clax on
/// [device]"), showing the code while the person approves it on the
/// remote's page, which [openAuth] opens (an in-app browser session where
/// the platform has one, [ExternalLinks.openAuth]).
///
/// It polls every interval the core gives, on [clock]. Answers the
/// connected remote; null when the person cancels or closes the sheet,
/// which forgets the waiting sign-in. A code older than
/// [connectCodeLifetime], or one the remote says expired, shows "That code
/// expired. Try again.". Once connected, [closeAuth] closes the browser
/// session.
Future<Remote?> showConnectSheet(
  BuildContext context,
  RemoteApi api, {
  String url = 'https://clax.dev',
  required Clock clock,
  String device = 'this device',
  Future<bool> Function(Uri url)? openAuth,
  Future<void> Function()? closeAuth,
}) {
  const links = ExternalLinks();
  return showBundleSheet<Remote>(
    context,
    ConnectSheet(
      api: api,
      url: url,
      clock: clock,
      device: device,
      openAuth: openAuth ?? links.openAuth,
      closeAuth: closeAuth ?? links.closeAuth,
    ),
  );
}

/// The sheet of [showConnectSheet]: starting, the code with **Open
/// `<host>` again** and **Cancel** (and the sign-in page's address to go
/// to by hand when it did not open), or why it stopped, with **Try again**
/// unless the answer was final (a hosted Clax too old for this app, or one
/// that refused the sign-in). Pops the connected [Remote].
class ConnectSheet extends StatefulWidget {
  const ConnectSheet({
    super.key,
    required this.api,
    required this.url,
    required this.clock,
    required this.device,
    required this.openAuth,
    required this.closeAuth,
  });

  final RemoteApi api;
  final String url;
  final Clock clock;
  final String device;
  final Future<bool> Function(Uri url) openAuth;
  final Future<void> Function() closeAuth;

  @override
  State<ConnectSheet> createState() => _ConnectSheetState();
}

class _ConnectSheetState extends State<ConnectSheet> {
  /// Each sign-in's run; a newer one (Try again, Cancel, closing) ends
  /// the older one's polling.
  var _run = 0;
  SignIn? _signIn;

  /// Why the last sign-in stopped; null while one starts or waits.
  String? _stopped;

  /// Whether starting again could end otherwise: false for the remote's
  /// final answers (too old, a refusal), which show no Try again.
  var _retryable = true;

  /// Whether the sign-in page opened, as [ConnectSheet.openAuth] last
  /// answered; null while it is opening.
  bool? _opened;
  var _connected = false;

  String get _host => Uri.parse(widget.url).host;
  String get _machine => machineName(widget.device);

  @override
  void initState() {
    super.initState();
    unawaited(_start());
  }

  @override
  void dispose() {
    _run++;
    _forget();
    super.dispose();
  }

  bool _live(int run) => mounted && run == _run;

  /// Forgets the sign-in still waiting, so the core keeps no pending
  /// remote.
  void _forget() {
    final s = _signIn;
    _signIn = null;
    if (s == null || _connected) return;
    unawaited(
      widget.api.removeRemote(s.remote.id).catchError((Object _) => false),
    );
  }

  void _stop(int run, String why, {bool retryable = true}) {
    if (!_live(run)) return;
    setState(() {
      _stopped = why;
      _retryable = retryable;
    });
  }

  /// Why a sign-in could not start, for [e]; and whether starting again
  /// could help.
  (String, bool) _notStarted(ClaxApiError e) {
    if (remoteUnreachable(e)) return (unreachableLine(_host), true);
    if (remoteTooOld(e)) return (tooOldLine(_host, 'sign in'), false);
    if (e.code == 'remote_refused' || remoteSignedOut(e)) {
      return ('$_host didn’t accept this device’s sign-in.', false);
    }
    if (e.code == 'invalid_args') {
      return ('$_host isn’t an address Clax can sign in to.', false);
    }
    return ('Couldn’t start signing in to $_host. Try again.', true);
  }

  /// Opens the sign-in page at [url], keeping whether it opened.
  Future<void> _openAuth(int run, Uri url) async {
    var ok = false;
    try {
      ok = await widget.openAuth(url);
    } on Object {
      ok = false;
    }
    if (_live(run)) setState(() => _opened = ok);
  }

  Future<void> _start() async {
    final run = ++_run;
    _forget();
    if (_stopped != null) setState(() => _stopped = null);
    final SignIn s;
    try {
      s = await widget.api.addRemote(widget.url, machine: _machine);
    } on ClaxApiError catch (e) {
      // Signed in there already (from another screen, or before Settings
      // last looked): that remote is the answer.
      if (e.code == 'remote_exists') {
        if (await _existing() case final r?) {
          if (!_live(run) || !mounted) return;
          _connected = true;
          Navigator.of(context).pop(r);
          return;
        }
      }
      final (why, retryable) = _notStarted(e);
      _stop(run, why, retryable: retryable);
      return;
    }
    if (!_live(run)) {
      unawaited(widget.api.removeRemote(s.remote.id).catchError((Object _) => false));
      return;
    }
    setState(() {
      _signIn = s;
      _opened = null;
    });
    final until = widget.clock.now().add(connectCodeLifetime);
    unawaited(_openAuth(run, s.verificationUrl));
    while (true) {
      await widget.clock.delay(Duration(seconds: s.interval));
      if (!_live(run)) return;
      if (!widget.clock.now().isBefore(until)) {
        _forget();
        _stop(run, _expired);
        return;
      }
      final Remote r;
      try {
        r = await widget.api.poll(s.remote.id);
      } on ClaxApiError catch (e) {
        // Offline, or the remote out of reach: ask again next time. Any
        // other answer (the core passes the remote's refusals on as 502)
        // is final.
        if (remoteUnreachable(e)) continue;
        _forget();
        if (remoteTooOld(e)) {
          _stop(run, tooOldLine(_host, 'sign in'), retryable: false);
        } else {
          _stop(run, 'Couldn’t finish signing in to $_host. Try again.');
        }
        return;
      }
      if (!_live(run)) {
        // Cancelled or closed while the poll was out: a sign-in it
        // finished is not kept.
        if (r.connected) {
          unawaited(widget.api.removeRemote(r.id).catchError((Object _) => false));
        }
        return;
      }
      switch (r.status) {
        case 'connected':
          _connected = true;
          unawaited(widget.closeAuth());
          if (!mounted) return;
          Navigator.of(context).pop(r);
          return;
        case 'denied':
          _signIn = null;
          _stop(run, 'This device was turned down on $_host.');
          return;
        case 'expired':
          _signIn = null;
          _stop(run, _expired);
          return;
      }
    }
  }

  static const _expired = 'That code expired. Try again.';

  /// The remote at [ConnectSheet.url] this device is signed in to, or
  /// null.
  Future<Remote?> _existing() async {
    final want = originOf(widget.url);
    try {
      for (final r in await widget.api.remotes()) {
        if (r.connected && originOf(r.origin) == want) return r;
      }
    } on ClaxApiError {
      // Not known: the sheet says it could not start.
    }
    return null;
  }

  /// Ends the sign-in, forgetting it, and closes the sheet with no remote.
  void _cancel() {
    _run++;
    _forget();
    unawaited(widget.closeAuth());
    Navigator.of(context).pop();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final muted = theme.textTheme.bodySmall?.copyWith(
      color: claxPalette(context).muted,
    );
    final s = _signIn;
    final stopped = _stopped;
    final cancel = ClaxButton(
      label: stopped != null && !_retryable ? 'Close' : 'Cancel',
      onPressed: _cancel,
    );
    return BundleSheetFrame(
      face: null,
      title: 'Connect to $_host',
      body: [
        if (stopped != null)
          Text(stopped, style: theme.textTheme.bodyMedium)
        else if (s == null)
          Text('Asking $_host for a code…', style: muted)
        else if (_opened == false) ...[
          Text(
            '$_host didn’t open. Go to this address on any device and sign '
            'in, then check that it shows the code below:',
            style: theme.textTheme.bodyMedium,
          ),
          const SizedBox(height: 8),
          Row(
            children: [
              Expanded(
                child: SelectableText(
                  '${s.verificationUrl}',
                  key: const ValueKey('connect-url'),
                  style: theme.textTheme.bodyMedium,
                ),
              ),
              IconButton(
                key: const ValueKey('connect-copy'),
                tooltip: 'Copy the address',
                icon: const Icon(Icons.copy_rounded, size: 20),
                onPressed: () => unawaited(
                  Clipboard.setData(
                    ClipboardData(text: '${s.verificationUrl}'),
                  ),
                ),
              ),
            ],
          ),
          _code(theme, s),
          Text(
            'This device will be listed as $_machine. You can disconnect '
            'it from either side at any time.',
            style: muted,
          ),
        ] else ...[
          Text(
            '$_host is open in your browser. Sign in there, then check '
            'that it shows this code:',
            style: theme.textTheme.bodyMedium,
          ),
          _code(theme, s),
          Text(
            'This device will be listed as $_machine. You can disconnect '
            'it from either side at any time.',
            style: muted,
          ),
        ],
      ],
      actions: [
        cancel,
        if (stopped != null && _retryable)
          ClaxButton(
            key: const ValueKey('connect-retry'),
            kind: ClaxButtonKind.primary,
            isDefault: true,
            label: 'Try again',
            onPressed: () => unawaited(_start()),
          )
        else if (s != null)
          ClaxButton(
            key: const ValueKey('connect-open'),
            kind: ClaxButtonKind.primary,
            isDefault: true,
            label: 'Open $_host again',
            onPressed: () => unawaited(_openAuth(_run, s.verificationUrl)),
          ),
      ],
    );
  }

  /// [s]'s code, large, to check against the remote's page.
  Widget _code(ThemeData theme, SignIn s) => Padding(
    padding: const EdgeInsets.symmetric(vertical: 16),
    child: Center(
      child: SelectableText(
        s.userCode,
        key: const ValueKey('connect-code'),
        style: theme.textTheme.headlineMedium?.copyWith(
          fontFeatures: const [FontFeature.tabularFigures()],
          letterSpacing: 2,
        ),
      ),
    ),
  );
}
