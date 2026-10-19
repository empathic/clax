import 'dart:typed_data';

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

  /// Whether the app shows the page: on screen, resumed, and not covered
  /// by another screen. The shell is told after each document's init and
  /// on every change.
  void setVisible(bool visible);

  /// The page's document failing to load (or its renderer going away).
  Stream<SurfaceFault> get faults;

  /// The artifact ID of the loaded top-level document, from the URL it
  /// was committed at (never from the page); null before it loaded.
  String? get document;

  /// Each other artifact the page's top level tried to navigate to. The
  /// surface keeps its top level on the artifact it was [load]ed with and
  /// refuses the navigation; the app opens that artifact as its own
  /// screen.
  Stream<String> get elsewhere;

  void dispose();
}

/// The page's document failed to load: [detail] is the WebView's raw
/// description (such as `net::ERR_CONNECTION_REFUSED`), for the log and
/// Developer tools; `userMessage` says it plainly. When [reloading], the
/// surface is loading it again by itself; otherwise it gave up and waits
/// for [AppSurface.reload].
class SurfaceFault {
  const SurfaceFault(this.detail, {required this.reloading});

  final String detail;
  final bool reloading;

  @override
  String toString() => 'SurfaceFault($detail, reloading: $reloading)';
}

/// An image of a surface's page: [png] at the device's pixels, of a view
/// [width] by [height] logical pixels.
typedef SurfaceShot = ({Uint8List png, double width, double height});

/// A surface that can take an image of its page as it shows now.
abstract interface class CapturableSurface {
  /// The page's image; null when the platform has none to give (no
  /// WebView yet, or a platform without the call).
  Future<SurfaceShot?> capture();
}
