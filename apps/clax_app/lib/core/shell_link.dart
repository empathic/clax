/// One WebView's shell on the core: its scheme, its loads' nonces, and the
/// relay of the load's calls.
library;

import 'dart:convert';

import 'channel_relay.dart';
import 'core_client.dart';
import 'origins.dart';
import 'scheme.dart';
import 'scheme_host.dart';

/// The nonces of one WebView's shell documents, kept by navigation number
/// (never by URL), and the load's.
///
/// Each shell document answered under a number joins that number's set:
/// on Apple a frame's `<img>` or `no-cors` fetch of the shell's URL while
/// the permit is open carries the number too, so one number can have
/// several. At a commit of number `n` set `n` is the load's candidates and
/// the sets of every number up to `n` are dropped; the first candidate the
/// main frame presents pins the load, and the other candidates are
/// dropped. A commit with no number, or with a number with no set, leaves
/// the load with none.
class ShellLoads {
  final Map<int, Set<String>> _sets = {};
  Set<String> _candidates = const {};
  String? _pinned;

  /// The load's nonce, once pinned.
  String? get pinned => _pinned;

  /// The scheme answered a shell document under [navigation] with
  /// [nonce].
  void minted(int navigation, String nonce) =>
      (_sets[navigation] ??= {}).add(nonce);

  /// A main-frame navigation committed: a new load, whose candidates are
  /// [navigation]'s set.
  void committed(int? navigation) {
    _pinned = null;
    _candidates = navigation == null ? const {} : _sets[navigation] ?? const {};
    if (navigation != null) _sets.removeWhere((n, _) => n <= navigation);
  }

  /// Numbered navigation [navigation] will not commit.
  void dropped(int navigation) => _sets.remove(navigation);

  /// Whether a message under [nonce] is the load's: the pinned nonce, or
  /// before the pin a candidate, which pins the load.
  bool present(Object? nonce) {
    if (nonce is! String) return false;
    if (_pinned case final p?) return p == nonce;
    if (!_candidates.contains(nonce)) return false;
    _pinned = nonce;
    _candidates = const {};
    return true;
  }
}

/// What a WebView's surface asks of its shell link: the load's nonce once
/// it is pinned, a nonce the document carries ([CoreShellLink.presented]),
/// and the end of the link.
abstract interface class ShellLinkPort {
  /// The load's nonce, once pinned.
  String? get nonce;

  /// The main frame's document carries [nonce] in its `clax-nonce` meta.
  void presented(String nonce);

  /// Ends the load's relay; nothing more is answered.
  void dispose();
}

/// [SchemeClient] for one WebView on the core: answers its scheme
/// requests with [SchemeAnswerer], keeps its loads' nonces ([ShellLoads]),
/// and hands each `ClaxHost` message of the load's nonce to the load's
/// [ChannelRelay] or, when it is not the relay's, to [onShellMessage] (the
/// surface's `ShellSession`). Every other message is dropped. The relay is
/// made when the load is pinned ([onPinned] hears its nonce, which
/// `claxHost.init` gets) and disposed at the next commit. A shell document
/// the scheme could not give a nonce is reported on [onFault].
class CoreShellLink implements SchemeClient, ShellLinkPort {
  CoreShellLink(
    this.core,
    AppOrigins origins, {
    required this.runJs,
    required this.onShellMessage,
    required this.onPinned,
    required this.onFault,
    this.onCommitted,
  }) : _scheme = SchemeAnswerer(core, origins, onFault: onFault);

  final CoreCalls core;

  /// Runs an expression in the WebView's document.
  final Future<void> Function(String js) runJs;

  /// A message of the load's nonce that is not the relay's, raw.
  final void Function(String raw) onShellMessage;

  /// The load's nonce, once the main frame presented it.
  final void Function(String nonce) onPinned;

  /// A fault the load cannot recover from by itself.
  final void Function(String detail) onFault;

  /// A main-frame navigation committed: the load has no nonce until one
  /// is pinned again.
  final void Function()? onCommitted;

  final SchemeAnswerer _scheme;
  final ShellLoads _loads = ShellLoads();
  ChannelRelay? _relay;
  bool _disposed = false;

  /// The load's nonce, once pinned.
  @override
  String? get nonce => _loads.pinned;

  @override
  Future<SchemeAnswer> request(SchemeRequest r) async {
    final a = await _scheme.answer(r);
    final n = r.navigation;
    if (a.nonce case final nonce? when n != null && !_disposed) {
      _loads.minted(n, nonce);
    }
    return a;
  }

  @override
  void committed(Uri? url, int? navigation) {
    _relay?.dispose();
    _relay = null;
    _loads.committed(navigation);
    onCommitted?.call();
  }

  @override
  void dropped(int navigation) => _loads.dropped(navigation);

  @override
  void refused(Uri? url) {}

  @override
  void channel(String message) {
    if (_disposed) return;
    final Map<String, Object?> m;
    try {
      final j = jsonDecode(message);
      if (j is! Map<String, Object?> || j['type'] is! String) return;
      m = j;
    } on FormatException {
      return;
    }
    if (!_present(m['nonce'])) return;
    if (_relay?.handle(m) ?? false) return;
    onShellMessage(message);
  }

  /// The main frame's document carries [nonce] in its `clax-nonce` meta
  /// (read once it has loaded): before the pin, a candidate pins the load
  /// as a message under it would.
  @override
  void presented(String nonce) {
    if (!_disposed && _loads.pinned == null) _present(nonce);
  }

  /// Whether [nonce] is the load's, pinning it (and making its relay) when
  /// it is the first candidate presented.
  bool _present(Object? nonce) {
    final pinned = _loads.pinned;
    if (!_loads.present(nonce)) return false;
    if (pinned == null) {
      _relay = ChannelRelay(core, runJs: runJs);
      onPinned(_loads.pinned!);
    }
    return true;
  }

  /// Ends the load's relay; nothing more is answered.
  @override
  void dispose() {
    _disposed = true;
    _relay?.dispose();
    _relay = null;
  }
}
