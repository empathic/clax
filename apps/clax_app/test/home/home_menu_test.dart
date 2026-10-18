import 'dart:async';

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/ui/launcher.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import '../support/fake_voice.dart';

const _page = {'index.html': '<!doctype html><title>x</title>'};

void main() {
  late FakeClaxApi api;
  late HomeHost host;
  late HomeFlows flows;
  late String app;
  late List<String> opened;
  final nav = GlobalKey<NavigatorState>();

  Future<void> pump(WidgetTester tester) async {
    api = FakeClaxApi();
    opened = [];
    final home = api.seedArtifact('Home', _page);
    app = api.seedArtifact('Runs', _page);
    flows = HomeFlows(
      contextOf: () => nav.currentContext,
      api: api,
      openApp: opened.add,
      voice: FakeVoice.unsupported('No speech here'),
    );
    host = HomeHost(
      api: api,
      stream: api.stream(),
      closeStream: true,
      makes: MakeService((r, c, s) => Completer<String>().future),
      builds: null,
      settings: HomeSettings(homeAppId: home),
      actions: flows,
    );
    flows.host = host;
    addTearDown(host.dispose);
    await tester.pumpWidget(
      MaterialApp(navigatorKey: nav, home: const Scaffold()),
    );
    await tester.runAsync(host.load);
    await tester.pump();
  }

  testWidgets(
    "the page's items come first, above a divider, and a pick resolves its ID",
    (tester) async {
      await pump(tester);
      final picked = flows.menu(app, Rect.zero, const [
        (id: 'star', label: 'Star'),
        (id: 'rm', label: 'Remove from group'),
      ]);
      await tester.pumpAndSettle();
      final star = tester.getTopLeft(find.text('Star'));
      final remove = tester.getTopLeft(find.text('Remove from group'));
      final open = tester.getTopLeft(find.text('Open'));
      expect(star.dy, lessThan(remove.dy));
      expect(remove.dy, lessThan(open.dy));
      expect(find.byType(Divider), findsOneWidget);
      await tester.tap(find.byKey(const ValueKey('home-item-star')));
      await tester.pumpAndSettle();
      expect(await picked, 'star');
      expect(opened, isEmpty);
    },
  );

  testWidgets(
    "Clax's own item acts and resolves null; a dismissal resolves null",
    (tester) async {
      await pump(tester);
      var picked = flows.menu(app, Rect.zero, const [
        (id: 'star', label: 'Star'),
      ]);
      await tester.pumpAndSettle();
      await tester.tap(find.text('Open'));
      await tester.pumpAndSettle();
      expect(await picked, isNull);
      expect(opened, [app]);
      picked = flows.menu(app, Rect.zero, const []);
      await tester.pumpAndSettle();
      expect(find.text('Open'), findsOneWidget);
      expect(find.byType(Divider), findsNothing);
      await tester.tapAt(const Offset(10, 10));
      await tester.pumpAndSettle();
      expect(find.text('Open'), findsNothing);
      expect(await picked, isNull);
    },
  );

  testWidgets('a menu for an app not listed resolves null and shows nothing', (
    tester,
  ) async {
    await pump(tester);
    final picked = flows.menu('nope', Rect.zero, const [
      (id: 'star', label: 'Star'),
    ]);
    await tester.pumpAndSettle();
    expect(find.text('Star'), findsNothing);
    expect(await picked, isNull);
  });
}
