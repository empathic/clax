import 'package:flutter/widgets.dart';

import 'host_protocol.dart';

/// The page of an app: the shell's app mode in a WebView
/// (`WebViewAppSurface`), or a fake in tests. The app screen that is given
/// a surface owns it and disposes it.
abstract interface class AppSurface {
  /// The view of the page, filling the space it is given.
  Widget build(BuildContext context);

  /// Loads [url] as the page's document.
  Future<void> load(Uri url);

  /// Loads the current document again.
  Future<void> reload();

  /// Calls `window.claxHost` in the page.
  Future<void> call(HostCall c);

  /// The shell's messages, in order.
  Stream<HostMessage> get messages;

  /// The page's document failing to load (or its renderer going away).
  Stream<SurfaceFault> get faults;

  void dispose();
}

/// The page's document failed to load: [description] says how. When
/// [reloading], the surface is loading it again by itself; otherwise it
/// gave up and waits for [AppSurface.reload].
class SurfaceFault {
  const SurfaceFault(this.description, {required this.reloading});

  final String description;
  final bool reloading;

  @override
  String toString() => 'SurfaceFault($description, reloading: $reloading)';
}
