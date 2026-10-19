/// Where a share sheet points from. An iPad shows the share sheet as a
/// popover, which UIKit refuses without a rect in the window to anchor it
/// to (`share_plus`'s `sharePositionOrigin`); phones ignore it.
library;

import 'dart:ui' as ui;

import 'package:flutter/widgets.dart';

/// The rect in the window of the widget [context] is for (the tapped
/// button's), or [screenCentreAnchor] when it is not laid out.
Rect shareAnchorOf(BuildContext context) {
  final box = context.findRenderObject();
  if (box is RenderBox && box.attached && box.hasSize && !box.size.isEmpty) {
    return box.localToGlobal(Offset.zero) & box.size;
  }
  return screenCentreAnchor(View.maybeOf(context));
}

/// A 2-point square at the centre of [view] (the app's first view when
/// null), for a share with no button to point from.
Rect screenCentreAnchor([ui.FlutterView? view]) {
  final v = view ?? ui.PlatformDispatcher.instance.views.firstOrNull;
  if (v == null) return const Rect.fromLTWH(0, 0, 2, 2);
  final size = v.physicalSize / v.devicePixelRatio;
  return Rect.fromCenter(center: size.center(Offset.zero), width: 2, height: 2);
}
