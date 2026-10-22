import 'dart:async';
import 'dart:io';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/platform/clock.dart';
import 'package:clax_app/platform/device/device_grants.dart';
import 'package:clax_app/platform/device/device_host.dart';
import 'package:clax_app/platform/device/reminders.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/app_screen.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/mac/mac_toolbar.dart';
import 'package:clax_app/ui/mac/window_channel.dart';
import 'package:clax_app/ui/pill_strip.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/working_chip.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart' show HitTestResult;
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import '../platform/device/device_host_test.dart'
    show
        FakeCamera,
        FakeConsent,
        FakeFiles,
        FakeLocation,
        FakeNotifier,
        FakeShare;
import '../support/fake_surface.dart';
import '../support/fake_voice.dart';

void main() {
  /// A phone's window: from medium width an app shows in a pane, with
  /// Location in its top app bar rather than the pill.
  void phone(WidgetTester tester) {
    tester.view.physicalSize = const Size(400, 800);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.reset);
  }

  Future<FakeAppSurface> pump(
    WidgetTester tester,
    DeviceHost? device, {
    Clock clock = const SystemClock(),
  }) async {
    phone(tester);
    final api = FakeClaxApi();
    final aid = api.seedArtifact('Plants', {'index.html': '<h1>Plants</h1>'});
    final surface = FakeAppSurface();
    await tester.pumpWidget(
      MaterialApp(
        home: AppScreen(
          aid: aid,
          base: Uri.parse('http://127.0.0.1:7999/'),
          api: api,
          surface: surface,
          voice: FakeVoice(),
          settings: AppSettings(),
          timings: const NoTimings(),
          worker: const Stream.empty(),
          device: device,
          clock: clock,
        ),
      ),
    );
    return surface;
  }

  testWidgets(
    'a device call is made by the device host and answered in the page',
    (tester) async {
      final consent = FakeConsent();
      final device = DeviceHost(
        grants: MemoryDeviceGrants(),
        consent: consent,
        reminders: Reminders(FakeNotifier()),
        share: FakeShare(),
        files: FakeFiles(),
        camera: FakeCamera(),
        location: FakeLocation(),
      );
      final surface = await pump(tester, device);
      surface.push(
        const DeviceRequest(
          id: 'd7',
          cap: 'location',
          method: 'current',
          args: [],
        ),
      );
      await tester.pump();
      await tester.pump();
      final result = surface.hostCalls.whereType<DeviceResult>().single;
      expect((result.id, result.code), ('d7', null));
      expect(result.value, {
        'lat': 52.5,
        'lon': 13.4,
        'accuracy': 12.0,
        'precise': true,
      });
      expect(consent.asked.single.$2, 'location');
    },
  );

  DeviceHost host(FakeLocation location) => DeviceHost(
    grants: MemoryDeviceGrants(),
    consent: FakeConsent(),
    reminders: Reminders(FakeNotifier()),
    share: FakeShare(),
    files: FakeFiles(),
    camera: FakeCamera(),
    location: location,
  );

  /// The Clax pill saying Location.
  final locationPill = find.byWidgetPredicate(
    (w) => w is ClaxPill && w.state == ClaxPillState.location,
  );

  /// Past a dialog's open or close; the loading cover never settles.
  Future<void> settle(WidgetTester tester) async {
    for (var i = 0; i < 6; i++) {
      await tester.pump(const Duration(milliseconds: 100));
    }
  }

  DeviceRequest watch(String id) => DeviceRequest(
    id: 'req-$id',
    cap: 'location',
    method: 'watch',
    args: [
      {'watch': id},
    ],
  );

  testWidgets(
    'a watch shows the location pill, sends updates, and stops for good '
    'when the app goes to the background',
    (tester) async {
      final location = FakeLocation();
      final surface = await pump(tester, host(location));
      surface.push(watch('w'));
      await tester.pump();
      await tester.pump();
      await tester.pump();
      expect(surface.hostCalls.whereType<DeviceResult>().single.value, {
        'watching': true,
      });
      expect(locationPill, findsOneWidget);
      location.move(52.5, 13.4);
      await tester.pump();
      expect(surface.hostCalls.whereType<DeviceEvent>().single.value, {
        'lat': 52.5,
        'lon': 13.4,
        'accuracy': 8.0,
        'precise': true,
      });

      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.paused);
      // Hidden and paused disable frames, so only a forced frame (the one
      // the system's app-switcher snapshot asks for) draws the screen now.
      tester.binding.scheduleForcedFrame();
      await tester.pump();
      final end = surface.hostCalls.whereType<DeviceEvent>().last;
      expect((end.watch, end.code), ('w', 'stopped'));
      expect(location.stream, isNull);
      expect(locationPill, findsNothing);

      // Back in front: no watch starts again by itself.
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.hidden);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      tester.binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
      await tester.pump();
      expect(location.stream, isNull);
      expect(location.asked, hasLength(1));
      expect(locationPill, findsNothing);
      // Past the pill's fold, which runs on the system clock here.
      await tester.pump(const Duration(seconds: 5));
    },
  );

  // The pill's screenshot, off by default; run with
  //   CLAX_L1_SHOTS=1 flutter test --update-goldens test/ui/app_screen_device_test.dart
  // and look at target/flutter-app/l1/location-pill-*.png.
  for (final dark in [false, true]) {
    final scheme = dark ? 'dark' : 'light';
    testWidgets(
      'location pill, phone, $scheme',
      skip: Platform.environment['CLAX_L1_SHOTS'] != '1',
      (tester) async {
        await tester.runAsync(() async {
          final dir =
              '${Platform.environment['FLUTTER_ROOT']}/bin/cache/artifacts/material_fonts';
          Future<ByteData> f(String n) async =>
              ByteData.sublistView(await File('$dir/$n').readAsBytes());
          await (FontLoader(
            'MaterialIcons',
          )..addFont(f('MaterialIcons-Regular.otf'))).load();
        });
        tester.view.physicalSize = const Size(400, 300);
        tester.view.devicePixelRatio = 1;
        tester.view.padding = const FakeViewPadding(top: 32);
        addTearDown(tester.view.reset);
        final api = FakeClaxApi();
        final aid = api.seedArtifact('Plants', {'index.html': '<h1>P</h1>'});
        final surface = FakeAppSurface();
        await tester.pumpWidget(
          MaterialApp(
            debugShowCheckedModeBanner: false,
            theme: claxTheme(dark ? Brightness.dark : Brightness.light, defaultTargetPlatform),
            home: AppScreen(
              aid: aid,
              base: Uri.parse('http://127.0.0.1:7999/'),
              api: api,
              surface: surface,
              voice: FakeVoice(),
              settings: AppSettings(),
              timings: const NoTimings(),
              worker: const Stream.empty(),
              device: host(FakeLocation()),
            ),
          ),
        );
        surface.push(watch('w'));
        for (var i = 0; i < 4; i++) {
          await tester.pump();
        }
        // Past the strip's slide.
        await tester.pump(const Duration(milliseconds: 300));
        await expectLater(
          find.byType(MaterialApp),
          matchesGoldenFile(
            '../../../../target/flutter-app/l1/location-pill-$scheme.png',
          ),
        );
      },
    );
  }

  for (final (name, pad) in const [
    ('a tall status bar', EdgeInsets.only(top: 48)),
    ('a cutout', EdgeInsets.only(top: 56)),
    ('a landscape cutout', EdgeInsets.only(top: 24, left: 44)),
  ]) {
    testWidgets('the location pill sits under the top inset and inside the '
        'side insets: $name', (tester) async {
      tester.view.physicalSize = const Size(560, 780);
      tester.view.devicePixelRatio = 1;
      tester.view.padding = FakeViewPadding(top: pad.top, left: pad.left);
      addTearDown(tester.view.reset);
      final api = FakeClaxApi();
      final aid = api.seedArtifact('Plants', {'index.html': '<h1>P</h1>'});
      final surface = FakeAppSurface();
      await tester.pumpWidget(
        MaterialApp(
          home: AppScreen(
            aid: aid,
            base: Uri.parse('http://127.0.0.1:7999/'),
            api: api,
            surface: surface,
            voice: FakeVoice(),
            settings: AppSettings(),
            timings: const NoTimings(),
            worker: const Stream.empty(),
            device: host(FakeLocation()),
          ),
        ),
      );
      surface.push(watch('w'));
      for (var i = 0; i < 4; i++) {
        await tester.pump();
      }
      // Past the strip's slide.
      await tester.pump(const Duration(milliseconds: 300));
      expect(locationPill, findsOneWidget);
      final pill = tester.getRect(find.byKey(const ValueKey('clax-pill')));
      expect(pill.top, pad.top + 8);
      expect(pill.left, greaterThanOrEqualTo(pad.left));
      await tester.pump(const Duration(seconds: 5));
    });
  }

  testWidgets('a tap on the location pill tells what uses it; Stop location '
      'stops it', (tester) async {
    final location = FakeLocation();
    final surface = await pump(tester, host(location));
    surface.push(watch('w'));
    await tester.pump();
    await tester.pump();
    await tester.pump();
    // Past the strip's slide.
    await tester.pump(const Duration(milliseconds: 300));
    expect(find.text('Location'), findsOneWidget);
    await tester.tap(locationPill);
    await settle(tester);
    expect(
      find.text('Plants is using this device’s location.'),
      findsOneWidget,
    );
    expect(location.stream, isNotNull, reason: 'telling stops nothing');
    await tester.tap(find.text('Stop location'));
    await settle(tester);
    expect(surface.hostCalls.whereType<DeviceEvent>().last.code, 'stopped');
    expect(location.stream, isNull);
    expect(locationPill, findsNothing);
    await tester.pump(const Duration(seconds: 5));
  });

  testWidgets(
    'location that Android will not ask for again offers Open settings',
    (tester) async {
      final location = FakeLocation()
        ..error = const DeviceError('system_denied', 'never again');
      final surface = await pump(tester, host(location));
      surface.push(
        const DeviceRequest(
          id: 'd1',
          cap: 'location',
          method: 'current',
          args: [],
        ),
      );
      await tester.pump();
      await tester.pump();
      expect(
        surface.hostCalls.whereType<DeviceResult>().single.code,
        'system_denied',
      );
      await tester.pump(const Duration(milliseconds: 500));
      expect(
        find.text('Location is off for Clax in Android settings'),
        findsOneWidget,
      );
      await tester.tap(find.text('Open settings'));
      await tester.pump();
      expect(location.opened, [LocationSettingsPage.app]);
      await tester.pump(const Duration(seconds: 5));
    },
  );

  group('on the Mac', () {
    late List<List<String>> items;

    void mockChannels(WidgetTester tester) {
      MacToolbar.reset();
      items = [];
      final m = tester.binding.defaultBinaryMessenger;
      m.setMockMethodCallHandler(MacToolbar.channel, (call) async {
        if (call.method == 'setItems') {
          final args = call.arguments as Map<Object?, Object?>;
          items.add([
            for (final i in args['items']! as List)
              [
                (i as Map<Object?, Object?>)['id'],
                if (i['on'] == true) 'on',
              ].join(' '),
          ]);
        }
        return null;
      });
      m.setMockMethodCallHandler(MacWindow.channel, (call) async => null);
      addTearDown(() {
        m.setMockMethodCallHandler(MacToolbar.channel, null);
        m.setMockMethodCallHandler(MacWindow.channel, null);
      });
    }

    Future<void> click(WidgetTester tester, String id) async {
      await tester.binding.defaultBinaryMessenger.handlePlatformMessage(
        MacToolbar.channel.name,
        MacToolbar.channel.codec.encodeMethodCall(
          MethodCall('clicked', {'view': tester.view.viewId, 'id': id}),
        ),
        (_) {},
      );
      await settle(tester);
    }

    testWidgets(
      'a watch shows Location in the window\'s toolbar, whatever the pill '
      'says; a click tells what uses it, and Stop location takes it away',
      (tester) async {
        mockChannels(tester);
        final location = FakeLocation();
        final surface = await pump(tester, host(location));
        await tester.pump();
        expect(items.last, isNot(contains('location on')));
        surface.push(watch('w'));
        await tester.pump();
        await tester.pump();
        await tester.pump();
        expect(items.last, contains('location on'));
        // Before Comment, after Back and Forward.
        expect(items.last.indexOf('location on'), 2);

        await click(tester, 'location');
        expect(
          find.text('Plants is using this device’s location.'),
          findsOneWidget,
        );
        await tester.tap(find.text('Stop location'));
        await settle(tester);
        expect(location.stream, isNull);
        expect(items.last, isNot(contains('location on')));
        await tester.pump(const Duration(seconds: 5));
      },
      variant: TargetPlatformVariant.only(TargetPlatform.macOS),
    );

    testWidgets(
      'location macOS will not ask for again names System Settings',
      (tester) async {
        mockChannels(tester);
        final location = FakeLocation()
          ..error = const DeviceError('system_denied', 'never again');
        final surface = await pump(tester, host(location));
        surface.push(
          const DeviceRequest(
            id: 'd1',
            cap: 'location',
            method: 'current',
            args: [],
          ),
        );
        await tester.pump();
        await tester.pump();
        await tester.pump(const Duration(milliseconds: 500));
        expect(
          find.text('Location is off for Clax in System Settings'),
          findsOneWidget,
        );
        await tester.pump(const Duration(seconds: 5));
      },
      variant: TargetPlatformVariant.only(TargetPlatform.macOS),
    );
  });

  testWidgets('without a device host every device call is unavailable', (
    tester,
  ) async {
    final surface = await pump(tester, null);
    surface.push(
      const DeviceRequest(id: 'd1', cap: 'camera', method: 'take', args: []),
    );
    await tester.pump();
    final result = surface.hostCalls.whereType<DeviceResult>().single;
    expect((result.id, result.code), ('d1', 'unavailable'));
  });

  test('the shell message parses into a DeviceRequest', () {
    final m = HostMessage.parse(
      '{"type":"device","id":"d2","cap":"notify","method":"cancel","args":["water"]}',
    );
    expect(m, isA<DeviceRequest>());
    final r = m! as DeviceRequest;
    expect((r.id, r.cap, r.method), ('d2', 'notify', 'cancel'));
    expect(r.args, ['water']);
    expect(
      HostMessage.parse('{"type":"device","id":"d2","cap":"notify"}'),
      isNull,
    );
    expect(
      const OpenRoute('#/plants').toJs(),
      'window.claxHost?.openRoute("#/plants")',
    );
  });

  testWidgets('in a pane on wide Android, Location is in the top app bar, '
      'and a tap on it tells what uses it', (tester) async {
    final surface = await pump(tester, host(FakeLocation()));
    tester.view.physicalSize = const Size(1280, 800);
    await tester.pump();
    surface.push(watch('w'));
    for (var i = 0; i < 4; i++) {
      await tester.pump();
    }
    expect(locationPill, findsNothing);
    final item = find.byKey(const ValueKey('pane-location'));
    expect(item, findsOneWidget);
    await tester.tap(item);
    await settle(tester);
    expect(find.text('Stop location'), findsOneWidget);
    // Past the toast's time.
    await tester.pump(const Duration(seconds: 10));
  });

  group('the pill\'s strip', () {
    final page = find.byKey(const ValueKey('fake-surface'));
    final strip = find.byKey(const ValueKey('pill-strip'));
    const top = 24.0;
    const full = Rect.fromLTRB(0, top, 400, 800);
    final open = Rect.fromLTRB(0, top + PillStrip.height, 400, 800);

    /// A phone with a 24 dp status bar, the screen on [clock].
    Future<FakeAppSurface> rig(WidgetTester tester, Clock clock) async {
      final surface = await pump(tester, host(FakeLocation()), clock: clock);
      tester.view.padding = const FakeViewPadding(top: top);
      // Ready, so the loading cover is not what keeps taps off the page.
      surface.push(
        const Ready(
          artifact: 'x',
          version: 1,
          latest: 1,
          file: 'index.html',
        ),
      );
      surface.push(const ChromeMsg(_head, Colors.white));
      await tester.pump();
      return surface;
    }

    /// The status bar's band's colour.
    Color band(WidgetTester tester) => tester
        .widget<ColoredBox>(find.byKey(const ValueKey('system-band-top')))
        .color;

    DeviceRequest unwatch(String id) => DeviceRequest(
      id: 'req-un-$id',
      cap: 'location',
      method: 'unwatch',
      args: [
        {'watch': id},
      ],
    );

    /// Lets the device host answer, then the strip slide (200 ms) end.
    Future<void> slide(WidgetTester tester) async {
      for (var i = 0; i < 3; i++) {
        await tester.pump();
      }
      await tester.pump(const Duration(milliseconds: 300));
    }

    testWidgets('while nothing goes on there is no strip and the page is at '
        'the top', (tester) async {
      await rig(tester, _StepClock());
      await slide(tester);
      expect(locationPill, findsNothing);
      expect(tester.getRect(page), full);
      expect(tester.getSize(strip).height, 0);
      expect(band(tester), _head);
    });

    testWidgets('the strip opens as the pill appears, and the page moves down '
        'by its height and is that much shorter', (tester) async {
      final surface = await rig(tester, _StepClock());
      surface.push(watch('w'));
      await tester.pump();
      await tester.pump();
      await tester.pump();
      // Sliding: the page moves whole, not yet resized.
      await tester.pump(const Duration(milliseconds: 100));
      final mid = tester.getRect(page);
      expect(mid.top, inExclusiveRange(top, top + PillStrip.height));
      expect(mid.height, full.height);
      await tester.pump(const Duration(milliseconds: 200));
      expect(tester.getRect(page), open);
      expect(
        tester.getRect(strip),
        const Rect.fromLTWH(0, top, 400, PillStrip.height),
      );
      // The pill and its whole target are in the strip, off the page.
      final pill = tester.getRect(find.byKey(const ValueKey('clax-pill')));
      final target = tester.getRect(
        find.byKey(const ValueKey('clax-pill-target')),
      );
      expect(pill.top, top + 8);
      expect(pill.right, 400 - 16);
      expect(target.top, greaterThanOrEqualTo(top));
      expect(target.bottom, lessThanOrEqualTo(open.top));
      // The status bar's band and the strip are one `bg` area.
      final bg = claxPalette(tester.element(strip)).bg;
      expect(band(tester), bg);
      expect(tester.widget<ColoredBox>(strip).color, bg);
    });

    testWidgets('folding to the mark does not move the page', (tester) async {
      final clock = _StepClock();
      final surface = await rig(tester, clock);
      surface.push(watch('w'));
      await slide(tester);
      expect(find.text('Location'), findsOneWidget);
      expect(tester.getRect(page), open);
      clock.advance(ClaxPill.foldAfter);
      for (var i = 0; i < 4; i++) {
        await tester.pump(const Duration(milliseconds: 100));
        expect(tester.getRect(page), open);
      }
      expect(find.text('Location'), findsNothing, reason: 'folded');
      expect(tester.getSize(find.byKey(const ValueKey('clax-pill'))), const Size(32, 32));
    });

    testWidgets('at 200% text the strip is as tall as the pill needs, open '
        'or folded', (tester) async {
      tester.platformDispatcher.textScaleFactorTestValue = 2;
      addTearDown(tester.platformDispatcher.clearTextScaleFactorTestValue);
      final clock = _StepClock();
      final surface = await rig(tester, clock);
      surface.push(watch('w'));
      await slide(tester);
      final pill = find.byKey(const ValueKey('clax-pill'));
      final target = find.byKey(const ValueKey('clax-pill-target'));
      final opened = tester.getRect(page);
      expect(tester.getSize(pill).height, greaterThan(32));
      expect(opened.top, greaterThan(top + PillStrip.height));
      expect(tester.getRect(strip).bottom, opened.top);
      for (final folded in [false, true]) {
        if (folded) {
          clock.advance(ClaxPill.foldAfter);
          await slide(tester);
          expect(find.text('Location'), findsNothing, reason: 'folded');
        }
        expect(tester.getRect(page), opened);
        expect(tester.getRect(target).top, greaterThanOrEqualTo(top));
        expect(tester.getRect(target).bottom, lessThanOrEqualTo(opened.top));
        // Centred in the strip.
        final s = tester.getRect(strip);
        expect(tester.getRect(pill).center.dy, closeTo(s.center.dy, 0.5));
      }
    });

    testWidgets('the strip closes once the last thing ends and the grace '
        'passes, and the page comes back up', (tester) async {
      final clock = _StepClock();
      final surface = await rig(tester, clock);
      surface.push(watch('w'));
      await slide(tester);
      expect(tester.getRect(page), open);
      surface.push(unwatch('w'));
      await slide(tester);
      expect(locationPill, findsNothing);
      expect(tester.getRect(page), open, reason: 'within the grace');
      clock.advance(PillStrip.grace);
      await tester.pump();
      await tester.pump();
      // Closing gives the page its height back first, then slides it up.
      await tester.pump(const Duration(milliseconds: 100));
      final mid = tester.getRect(page);
      expect(mid.top, inExclusiveRange(top, top + PillStrip.height));
      expect(mid.height, full.height);
      await tester.pump(const Duration(milliseconds: 200));
      expect(tester.getRect(page), full);
      expect(tester.getSize(strip).height, 0);
      await tester.pump();
      expect(band(tester), _head, reason: 'the page\'s own colour again');
    });

    testWidgets('a pill that goes and comes back within the grace keeps the '
        'page still', (tester) async {
      final clock = _StepClock();
      final surface = await rig(tester, clock);
      surface.push(watch('w'));
      await slide(tester);
      expect(tester.getRect(page), open);
      surface.push(unwatch('w'));
      await slide(tester);
      clock.advance(PillStrip.grace ~/ 2);
      await tester.pump();
      expect(tester.getRect(page), open);
      surface.push(watch('w2'));
      for (var i = 0; i < 3; i++) {
        await tester.pump();
      }
      expect(locationPill, findsOneWidget);
      clock.advance(PillStrip.grace);
      for (var i = 0; i < 6; i++) {
        await tester.pump(const Duration(milliseconds: 100));
        expect(tester.getRect(page), open);
      }
    });

    testWidgets('a pill that comes back as the strip starts to close leaves '
        'the page open and laid out shorter', (tester) async {
      final clock = _StepClock();
      final surface = await rig(tester, clock);
      surface.push(watch('w'));
      await slide(tester);
      surface.push(unwatch('w'));
      await slide(tester);
      // The grace ends and the close starts; before it moves the page,
      // the pill is back.
      clock.advance(PillStrip.grace);
      await tester.pump();
      surface.push(watch('w2'));
      for (var i = 0; i < 3; i++) {
        await tester.pump();
      }
      expect(locationPill, findsOneWidget);
      for (var i = 0; i < 6; i++) {
        await tester.pump(const Duration(milliseconds: 100));
        expect(tester.getRect(page), open);
      }
    });

    testWidgets('with reduced motion the strip opens and closes at once', (
      tester,
    ) async {
      tester.platformDispatcher.accessibilityFeaturesTestValue =
          const FakeAccessibilityFeatures(disableAnimations: true);
      addTearDown(tester.platformDispatcher.clearAccessibilityFeaturesTestValue);
      final clock = _StepClock();
      final surface = await rig(tester, clock);
      surface.push(watch('w'));
      for (var i = 0; i < 3; i++) {
        await tester.pump();
      }
      expect(locationPill, findsOneWidget);
      expect(tester.getRect(page), open);
      surface.push(unwatch('w'));
      for (var i = 0; i < 3; i++) {
        await tester.pump();
      }
      clock.advance(PillStrip.grace);
      await tester.pump();
      await tester.pump();
      expect(tester.getRect(page), full);
    });

    testWidgets('a tap on the strip beside the pill or the mark does nothing, '
        'and reaches no page', (tester) async {
      final clock = _StepClock();
      final surface = await rig(tester, clock);
      surface.push(watch('w'));
      await slide(tester);
      final details = find.text('Plants is using this device’s location.');
      final surfaceBox = tester.renderObject(page);
      Future<void> tapBeside(double x) async {
        final at = Offset(x, top + PillStrip.height / 2);
        final hit = HitTestResult();
        tester.binding.hitTestInView(hit, at, tester.view.viewId);
        expect(
          hit.path.map((e) => e.target),
          isNot(contains(surfaceBox)),
          reason: 'at $at',
        );
        await tester.tapAt(at);
        await settle(tester);
        expect(details, findsNothing, reason: 'at $at');
      }

      final target = tester.getRect(
        find.byKey(const ValueKey('clax-pill-target')),
      );
      await tapBeside(20);
      await tapBeside(target.left - 4);
      clock.advance(ClaxPill.foldAfter);
      await settle(tester);
      expect(find.text('Location'), findsNothing, reason: 'folded');
      final mark = tester.getRect(
        find.byKey(const ValueKey('clax-pill-target')),
      );
      expect(mark.width, 48);
      await tapBeside(mark.left - 4);
      await tapBeside(200);
      expect(surface.hostCalls.whereType<DeviceEvent>(), isEmpty);
      // Just under the strip, the page takes the tap.
      final under = HitTestResult();
      tester.binding.hitTestInView(
        under,
        Offset(200, top + PillStrip.height + 4),
        tester.view.viewId,
      );
      expect(under.path.map((e) => e.target), contains(surfaceBox));
    });

    testWidgets('the folded mark in the strip keeps the pill\'s action', (
      tester,
    ) async {
      final clock = _StepClock();
      final surface = await rig(tester, clock);
      surface.push(watch('w'));
      await slide(tester);
      clock.advance(ClaxPill.foldAfter);
      await settle(tester);
      expect(find.text('Location'), findsNothing, reason: 'folded');
      await tester.tap(locationPill);
      await settle(tester);
      expect(
        find.text('Plants is using this device’s location.'),
        findsOneWidget,
      );
      await tester.tap(find.text('Stop location'));
      await settle(tester);
      expect(surface.hostCalls.whereType<DeviceEvent>().last.code, 'stopped');
      expect(locationPill, findsNothing);
      clock.advance(PillStrip.grace);
      await slide(tester);
      expect(tester.getRect(page), full);
      // Past the toast's time.
      clock.advance(const Duration(seconds: 10));
      await tester.pump(const Duration(seconds: 10));
    });
  });
}

/// The page's header colour, which the status bar's band takes.
const _head = Color(0xFF102A4C);

/// A clock whose time moves only by [advance], which ends each delay that
/// falls due on the way.
class _StepClock implements Clock {
  DateTime t = DateTime.utc(2026, 10, 12);
  final _delays = <(DateTime, Completer<void>)>[];
  @override
  DateTime now() => t;
  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _delays.add((t.add(d), c));
    return c.future;
  }

  void advance(Duration d) {
    t = t.add(d);
    for (final (at, c) in List.of(_delays)) {
      if (!at.isAfter(t)) {
        c.complete();
        _delays.remove((at, c));
      }
    }
  }
}
