import 'dart:convert';
import 'dart:io';

import 'package:clax_app/ui/licences.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

/// The bundle's NOTICES.Z (//apps/clax_app:notices, as the app bundles
/// it), parsed as [ServicesBinding] parses it in the app, where
/// flutter_test registers none: entries split by a line of 80 dashes, each
/// its package names, a blank line, and the text.
Future<List<LicenseEntry>> bundledNotices() async {
  final data = await rootBundle.load('NOTICES.Z');
  final raw = utf8.decode(
    gzip.decode(data.buffer.asUint8List(data.offsetInBytes, data.lengthInBytes)),
  );
  return [
    for (final e in raw.split('\n${'-' * 80}\n'))
      if (e.indexOf('\n\n') case final i when i >= 0)
        LicenseEntryWithLineBreaks(e.substring(0, i).split('\n'), e.substring(i + 2)),
  ];
}

var _noticesAdded = false;

void main() {
  testWidgets("the licence registry holds Clax's own GPL-2.0-only notice", (
    tester,
  ) async {
    registerClaxLicence();
    // A second registration adds nothing.
    registerClaxLicence();
    final clax = await tester.runAsync(
      () => LicenseRegistry.licenses
          .where((l) => l.packages.contains('Clax'))
          .toList(),
    );
    expect(clax, hasLength(1));
    final text = clax!.single.paragraphs.map((p) => p.text).join('\n');
    expect(text, startsWith('Clax app (apps/clax_app)'));
    expect(text, contains('Copyright (C) 2026 Empathic, Inc.'));
    expect(text, contains('version 2 only (GPL-2.0-only)'));
    expect(text, contains('GNU GENERAL PUBLIC LICENSE'));
    expect(text, contains('Version 2, June 1991'));
  });

  testWidgets(
    "the bundle's NOTICES holds the third-party notices: pub packages, the engine and the embedded clax's crates",
    (tester) async {
      final packages = await tester.runAsync(() async {
        final names = <String>{};
        for (final l in await bundledNotices()) {
          names.addAll(l.packages);
        }
        return names;
      });
      for (final p in [
        'webview_flutter',
        'archive',
        'flutter',
        'skia',
        'ring',
        'tokio',
      ]) {
        expect(packages, contains(p));
      }
      expect(packages!.length, greaterThan(200));
    },
  );

  testWidgets('the licences page lists the third-party packages', (
    tester,
  ) async {
    final notices = await tester.runAsync(bundledNotices);
    if (!_noticesAdded) {
      _noticesAdded = true;
      LicenseRegistry.addLicense(() => Stream.fromIterable(notices!));
    }
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () => showClaxLicences(context),
            child: const Text('Licences'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Licences'));
    // The page reads the registry off the main isolate.
    final rows = find.textContaining(RegExp(r'^\d+ licenses?\.$'));
    for (var i = 0; i < 100 && rows.evaluate().isEmpty; i++) {
      await tester.runAsync(
        () => Future<void>.delayed(const Duration(milliseconds: 50)),
      );
      await tester.pump();
    }
    expect(rows, findsWidgets);
    await tester.scrollUntilVisible(
      find.text('webview_flutter'),
      400,
      scrollable: find.byType(Scrollable).last,
      maxScrolls: 200,
    );
    expect(find.text('webview_flutter'), findsOneWidget);
  });

  testWidgets('the licences page names Clax and its copyright', (
    tester,
  ) async {
    await tester.pumpWidget(
      MaterialApp(
        home: Builder(
          builder: (context) => TextButton(
            onPressed: () => showClaxLicences(context),
            child: const Text('Licences'),
          ),
        ),
      ),
    );
    await tester.tap(find.text('Licences'));
    await tester.pump();
    await tester.pump(const Duration(seconds: 1));
    expect(find.byType(LicensePage), findsOneWidget);
    expect(find.text('Clax'), findsWidgets);
    expect(find.textContaining('Copyright (C) 2026 Empathic, Inc.'), findsOneWidget);
  });
}
