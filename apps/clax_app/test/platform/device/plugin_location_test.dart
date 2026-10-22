import 'dart:async';

import 'package:clax_app/platform/device/device_plugins.dart';
import 'package:clax_app/platform/device/device_ports.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:geolocator/geolocator.dart';

/// geolocator's platform side, answering as told and keeping what it was
/// asked.
class FakeGeolocator extends GeolocatorPlatform {
  var serviceOn = true;
  var permission = LocationPermission.always;
  LocationPermission? afterRequest;
  var accuracyStatus = LocationAccuracyStatus.precise;
  final requests = <String>[];
  final settings = <LocationSettings>[];
  final opened = <String>[];
  final positions = StreamController<Position>();

  static Position at(double lat, double lon, double accuracy) => Position(
    latitude: lat,
    longitude: lon,
    accuracy: accuracy,
    timestamp: DateTime.utc(2026, 10, 19),
    altitude: 0,
    altitudeAccuracy: 0,
    heading: 0,
    headingAccuracy: 0,
    speed: 0,
    speedAccuracy: 0,
  );

  @override
  Future<bool> isLocationServiceEnabled() async => serviceOn;

  @override
  Future<LocationPermission> checkPermission() async => permission;

  @override
  Future<LocationPermission> requestPermission() async {
    requests.add('permission');
    return permission = afterRequest ?? permission;
  }

  @override
  Future<LocationAccuracyStatus> getLocationAccuracy() async => accuracyStatus;

  @override
  Future<LocationAccuracyStatus> requestTemporaryFullAccuracy({
    required String purposeKey,
  }) async {
    requests.add('temporary $purposeKey');
    return accuracyStatus;
  }

  @override
  Future<Position> getCurrentPosition({
    LocationSettings? locationSettings,
  }) async {
    settings.add(locationSettings!);
    return at(52.5, 13.4, 12);
  }

  @override
  Stream<Position> getPositionStream({LocationSettings? locationSettings}) {
    settings.add(locationSettings!);
    return positions.stream;
  }

  @override
  Future<bool> openAppSettings() async {
    opened.add('app');
    return true;
  }

  @override
  Future<bool> openLocationSettings() async {
    opened.add('location');
    return true;
  }
}

void main() {
  group('PluginLocation on macOS', () {
    late FakeGeolocator geo;
    late PluginLocation location;
    setUp(() {
      geo = FakeGeolocator();
      location = PluginLocation(
        platform: TargetPlatform.macOS,
        geolocator: geo,
      );
    });

    Future<DeviceError> refusal(Future<Object?> call) async {
      try {
        await call;
      } on DeviceError catch (e) {
        return e;
      }
      fail('the call did not reject');
    }

    test('current gives a precise fix when asked precise, at high accuracy '
        'with a time limit', () async {
      final fix = await location.current(Precision.precise);
      expect((fix.lat, fix.lon, fix.accuracy, fix.precise), (
        52.5,
        13.4,
        12.0,
        true,
      ));
      expect(geo.settings.single.accuracy, LocationAccuracy.high);
      expect(geo.settings.single.timeLimit, const Duration(seconds: 20));
      // The Mac has no reduced accuracy to lift: nothing more is asked.
      expect(geo.requests, isEmpty);
    });

    test('an approximate request asks low power and is never precise, '
        'which the device host coarsens', () async {
      final fix = await location.current(Precision.approximate);
      expect(fix.precise, isFalse);
      expect(geo.settings.single.accuracy, LocationAccuracy.low);
    });

    test('asks for the permission while macOS still may, and a refusal is '
        'not_granted', () async {
      geo
        ..permission = LocationPermission.denied
        ..afterRequest = LocationPermission.always;
      expect((await location.current(Precision.precise)).precise, isTrue);
      expect(geo.requests, ['permission']);

      geo
        ..permission = LocationPermission.denied
        ..afterRequest = LocationPermission.denied;
      expect((await refusal(location.current(Precision.precise))).code,
          'not_granted');
    });

    test('a denial macOS will not ask again is system_denied, naming macOS '
        'and System Settings', () async {
      geo.permission = LocationPermission.deniedForever;
      final e = await refusal(location.current(Precision.precise));
      expect(e.code, 'system_denied');
      expect(e.message, contains('macOS'));
      expect(e.message, contains('System Settings'));
      expect(e.message, isNot(contains('Android')));
    });

    test('location off on the Mac is location_off', () async {
      geo.serviceOn = false;
      expect((await refusal(location.current(Precision.precise))).code,
          'location_off');
      expect(
        (await refusal(location.watch(Precision.precise, minDistance: 10)))
            .code,
        'location_off',
      );
    });

    test('watch follows the device with Apple settings at the distance '
        'asked, and maps its errors', () async {
      final stream = await location.watch(Precision.precise, minDistance: 25);
      final s = geo.settings.single;
      expect(s, isA<AppleSettings>());
      expect(s.accuracy, LocationAccuracy.high);
      expect(s.distanceFilter, 25);
      expect((s as AppleSettings).allowBackgroundLocationUpdates, isFalse);

      final fixes = <LocationFix>[];
      final errors = <DeviceError>[];
      final sub = stream.listen(
        fixes.add,
        onError: (Object e) => errors.add(e as DeviceError),
      );
      geo.positions
        ..add(FakeGeolocator.at(1, 2, 5))
        ..addError(const LocationServiceDisabledException())
        ..addError(const PermissionDeniedException('no'));
      await pumpEventQueue();
      expect(fixes.single.toString(), 'LocationFix(1.0, 2.0, ±5.0, precise: true)');
      expect(errors.map((e) => e.code), ['location_off', 'not_granted']);
      await sub.cancel();
    });

    test('Open settings opens the Mac\'s location privacy settings',
        () async {
      await location.openSettings(LocationSettingsPage.app);
      await location.openSettings(LocationSettingsPage.location);
      expect(geo.opened, ['app', 'location']);
    });
  });

  test('a platform without location answers unavailable and opens nothing',
      () async {
    final geo = FakeGeolocator();
    final location = PluginLocation(
      platform: TargetPlatform.linux,
      geolocator: geo,
    );
    await expectLater(
      location.current(Precision.precise),
      throwsA(isA<DeviceError>().having((e) => e.code, 'code', 'unavailable')),
    );
    await location.openSettings(LocationSettingsPage.app);
    expect(geo.opened, isEmpty);
  });
}
