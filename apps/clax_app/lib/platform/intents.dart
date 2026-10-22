import 'dart:async';

import 'package:flutter/services.dart';

/// The App Intents the person runs in Clax from outside it: the Action
/// Button, Control Center, Siri or Shortcuts. Each is named by a word,
/// [comment] the only one so far.
abstract interface class Intents {
  /// "Comment in Clax": comment mode on the app on top, or on the app used
  /// last.
  static const comment = 'comment';

  /// Each intent run once [initial] has been asked, as it runs.
  /// Single-subscription; what arrives before the listener is kept for it.
  Stream<String> get intents;

  /// The intents run before Dart asked, oldest first: the one that
  /// launched Clax, if any. Asked once.
  Future<List<String>> initial();
}

/// [Intents] over the `dev.empathic.clax/intents` channel, served by
/// `ios/Runner/IntentsChannel.swift` over `ClaxIntents`
/// (`ios/Intents/CommentInClax.swift`).
///
/// Dart to platform: `initial` → a list of intent names, those run so far;
/// after it the platform sends each later one itself. Platform to Dart:
/// `intent` with the intent's name.
class ChannelIntents implements Intents {
  ChannelIntents({this._channel = const MethodChannel(channelName)}) {
    _channel.setMethodCallHandler(_onCall);
  }

  static const channelName = 'dev.empathic.clax/intents';

  final MethodChannel _channel;
  final _intents = StreamController<String>();

  @override
  Future<List<String>> initial() async {
    Object? names;
    try {
      names = await _channel.invokeMethod<Object?>('initial');
    } on PlatformException {
      return const [];
    } on MissingPluginException {
      return const [];
    }
    return [
      if (names is List)
        for (final n in names)
          if (n is String && n.isNotEmpty) n,
    ];
  }

  Future<Object?> _onCall(MethodCall call) async {
    if (call.method == 'intent') {
      if (call.arguments case final String n when n.isNotEmpty) {
        _intents.add(n);
      }
    }
    return null;
  }

  @override
  Stream<String> get intents => _intents.stream;
}

/// [Intents] for tests: [launch] stands in for the intents run before
/// Dart asked, [run] for one run later.
class FakeIntents implements Intents {
  /// What [initial] answers.
  List<String> launch = const [];
  final _intents = StreamController<String>();

  /// The person ran the intent [name].
  void run(String name) => _intents.add(name);

  @override
  Future<List<String>> initial() async => launch;

  @override
  Stream<String> get intents => _intents.stream;
}
