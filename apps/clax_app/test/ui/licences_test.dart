import 'package:clax_app/ui/licences.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

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
