import 'dart:async';

import 'package:clax_app/ui/app_surface.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:flutter/material.dart';

/// An [AppSurface] for widget tests: it records what the screen asks of the
/// page, and the test plays the shell's side ([push], [fault]).
class FakeAppSurface implements AppSurface {
  /// The URLs [load] was given, in order.
  final List<Uri> loads = [];

  /// How many times [reload] was called.
  int reloads = 0;

  /// Every call, attestations included, in order.
  final List<HostCall> calls = [];

  bool disposed = false;

  final _messages = StreamController<HostMessage>.broadcast(sync: true);
  final _faults = StreamController<SurfaceFault>.broadcast(sync: true);

  /// The calls other than [AttestPointer], in order.
  List<HostCall> get hostCalls => [
    for (final c in calls)
      if (c is! AttestPointer) c,
  ];

  /// The [AttestPointer] calls, in order.
  List<AttestPointer> get attests => calls.whereType<AttestPointer>().toList();

  /// The JavaScript of [hostCalls], which compares by value.
  List<String> get js => [for (final c in hostCalls) c.toJs()];

  /// Posts [m] as the shell would.
  void push(HostMessage m) => _messages.add(m);

  /// Reports a failed load as the WebView surface would.
  void fault(SurfaceFault f) => _faults.add(f);

  @override
  Widget build(BuildContext context) => const ColoredBox(
    key: ValueKey('fake-surface'),
    color: Color(0xFFEFEFEF),
    child: SizedBox.expand(),
  );

  @override
  Future<void> load(Uri url) async => loads.add(url);

  @override
  Future<void> reload() async => reloads++;

  @override
  Future<void> call(HostCall c) async => calls.add(c);

  @override
  Stream<HostMessage> get messages => _messages.stream;

  @override
  Stream<SurfaceFault> get faults => _faults.stream;

  @override
  void dispose() => disposed = true;
}
