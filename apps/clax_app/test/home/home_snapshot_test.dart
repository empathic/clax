import 'dart:async';
import 'dart:io';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/daemon/clock.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_route.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/home/home_snapshot.dart';
import 'package:clax_app/ui/app_surface.dart';
import 'package:clax_app/ui/make_service.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

const _page = {'index.html': '<h1>x</h1>'};

/// A clock whose delays complete as the test advances it.
class _Clock implements Clock {
  var _t = Duration.zero;
  final _waits = <(Duration, Completer<void>)>[];

  @override
  DateTime now() => DateTime.utc(2026, 10, 16).add(_t);

  @override
  Future<void> delay(Duration d) {
    final c = Completer<void>();
    _waits.add((_t + d, c));
    return c.future;
  }

  void advance(Duration d) {
    _t += d;
    for (final w in _waits.where((w) => w.$1 <= _t).toList()) {
      _waits.remove(w);
      w.$2.complete();
    }
  }
}

class _Actions implements HomeActions {
  final opened = <String>[];
  @override
  void open(String id) => opened.add(id);
  @override
  void openMake(String id) {}
  @override
  void newApp() {}
  @override
  void menu(String id, Rect anchor) {}
  @override
  void openInbox() {}
  @override
  void openSettings() {}
  @override
  void importApp() {}
}

/// A small PNG, made in the engine.
Future<Uint8List> _png() async {
  final rec = ui.PictureRecorder();
  Canvas(rec).drawRect(
    const Rect.fromLTWH(0, 0, 8, 6),
    Paint()..color = const Color(0xff3366cc),
  );
  final image = await rec.endRecording().toImage(8, 6);
  final data = await image.toByteData(format: ui.ImageByteFormat.png);
  image.dispose();
  return data!.buffer.asUint8List();
}

void main() {
  late Directory dir;

  setUp(() => dir = Directory.systemTemp.createTempSync('home-snapshot-'));
  tearDown(() => dir.deleteSync(recursive: true));

  HomeSnapshot snap(
    String aid,
    Uint8List png, {
    List<HomeTile> tiles = const [],
    ui.Image? image,
    double width = 800,
    double height = 600,
  }) => HomeSnapshot(
    aid: aid,
    png: png,
    width: width,
    height: height,
    tiles: tiles,
    at: DateTime.utc(2026, 10, 16, 9),
    image: image,
  );

  group('the snapshot store', () {
    test('keeps a snapshot and gives it back', () async {
      final store = HomeSnapshotStore(dir);
      final png = Uint8List.fromList(List.generate(300, (i) => i % 256));
      const tiles = [
        HomeTile('a1', Rect.fromLTWH(16, 80, 88, 104)),
        HomeTile('a2', Rect.fromLTWH(112.5, 80, 88, 104)),
      ];
      await store.save(snap('home1', png, tiles: tiles, width: 390));
      final back = (await store.load(decode: false))!;
      expect(back.aid, 'home1');
      expect(back.png, png);
      expect(back.width, 390);
      expect(back.height, 600);
      expect(back.tiles, tiles);
      expect(back.at, DateTime.utc(2026, 10, 16, 9));

      // A later save replaces it.
      await store.save(snap('home1', Uint8List.fromList([1, 2, 3])));
      expect((await store.load(decode: false))!.png, [1, 2, 3]);
    });

    testWidgets('decodes the image as it loads', (tester) async {
      final store = HomeSnapshotStore(dir);
      final loaded = await tester.runAsync(() async {
        await store.save(snap('home1', await _png()));
        return store.load();
      });
      expect(loaded!.image, isNotNull);
      expect(loaded.image!.width, 8);
    });

    test('a missing, corrupt or cut-short file is no snapshot', () async {
      final store = HomeSnapshotStore(dir);
      expect(await store.load(decode: false), isNull);
      final file = File('${dir.path}/home-snapshot.bin');
      file.writeAsStringSync('not a snapshot');
      expect(await store.load(decode: false), isNull);
      await store.save(snap('home1', Uint8List.fromList(List.filled(64, 7))));
      final whole = file.readAsBytesSync();
      file.writeAsBytesSync(whole.sublist(0, whole.length - 10));
      expect(await store.load(decode: false), isNull);
      file.writeAsBytesSync([...'{"aid":3}'.codeUnits, 0x0a, 1, 2]);
      expect(await store.load(decode: false), isNull);
    });

    testWidgets('a file whose image cannot be decoded is no snapshot', (
      tester,
    ) async {
      final store = HomeSnapshotStore(dir);
      final loaded = await tester.runAsync(() async {
        await store.save(snap('home1', Uint8List.fromList([1, 2, 3, 4])));
        return store.load();
      });
      expect(loaded, isNull);
    });
  });

  group('taking the snapshot', () {
    late _Clock clock;
    late HomeSnapshotStore store;
    late List<HomeTile> tiles;
    late bool inFront;
    late int captures;
    SurfaceShot? shot;

    HomeSnapshotter snapshotter() => HomeSnapshotter(
      store: store,
      clock: clock,
      inFront: () => inFront,
      capture: () async {
        captures++;
        final s = shot;
        return s == null ? null : (shot: s, aid: 'home1', tiles: tiles);
      },
    );

    setUp(() {
      clock = _Clock();
      store = HomeSnapshotStore(dir);
      tiles = const [HomeTile('a1', Rect.fromLTWH(0, 0, 10, 10))];
      inFront = true;
      captures = 0;
      shot = (png: Uint8List.fromList([9, 9]), width: 390.0, height: 844.0);
    });

    test(
      'a burst of tile reports takes one snapshot, once they settle',
      () async {
        final s = snapshotter();
        s.tilesChanged();
        clock.advance(const Duration(milliseconds: 500));
        tiles = const [HomeTile('a2', Rect.fromLTWH(0, 0, 20, 20))];
        s.tilesChanged();
        clock.advance(const Duration(milliseconds: 500));
        await pumpEventQueue();
        expect(captures, 0);
        clock.advance(const Duration(milliseconds: 300));
        await pumpEventQueue();
        await s.idle;
        expect(captures, 1);
        final kept = (await store.load(decode: false))!;
        expect(kept.tiles.single.id, 'a2');
        expect(kept.width, 390);
        expect(kept.png, [9, 9]);
      },
    );

    test('a report while the home screen is not in front takes none', () async {
      final s = snapshotter();
      inFront = false;
      s.tilesChanged();
      clock.advance(const Duration(seconds: 1));
      await pumpEventQueue();
      expect(captures, 0);
      // Covered or put away: taken at once.
      await s.take();
      expect(captures, 1);
      expect(await store.load(decode: false), isNotNull);
    });

    test('no image to take keeps the last snapshot', () async {
      final s = snapshotter();
      await s.take();
      shot = null;
      await s.take();
      expect(captures, 2);
      expect((await store.load(decode: false))!.png, [9, 9]);
    });
  });

  group('the cold start cover', () {
    late FakeClaxApi api;
    late String home;
    late String notes;
    late String photos;
    late HomeHost host;
    late _Actions actions;
    late _Clock clock;

    setUp(() {
      var t = 0;
      api = FakeClaxApi(
        clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
      );
      home = api.seedArtifact('Home', _page, capabilities: {'home': {}});
      notes = api.seedArtifact('Notes', _page);
      photos = api.seedArtifact('Photos', _page);
      actions = _Actions();
      clock = _Clock();
      host = HomeHost(
        api: api,
        stream: api.stream(),
        closeStream: true,
        makes: MakeService((r, c, s) => Completer<String>().future),
        builds: null,
        settings: HomeSettings(homeAppId: home),
        actions: actions,
      );
    });

    tearDown(() => host.dispose());

    /// The snapshot as the test screen (800 by 600) shows it: Notes'
    /// tile at the top left, Photos' beside it.
    HomeSnapshotCover cover({ui.Image? image, String? aid}) =>
        HomeSnapshotCover(
          snap(
            aid ?? home,
            Uint8List(0),
            image: image,
            tiles: [
              HomeTile(notes, const Rect.fromLTWH(0, 0, 100, 100)),
              HomeTile(photos, const Rect.fromLTWH(100, 0, 100, 100)),
            ],
          ),
        );

    Future<void> pump(WidgetTester tester, HomeSnapshotCover c) =>
        tester.pumpWidget(
          MaterialApp(
            home: HomeRoute(
              host: host,
              clock: clock,
              snapshot: c,
              app: (context, aid, hooks) => Text('page $aid'),
              fallback: (_, _) => const Text('built-in'),
            ),
          ),
        );

    Future<void> shown(WidgetTester tester) async {
      await tester.runAsync(() => host.call(home, 'shown', const {}));
      await tester.pump();
    }

    testWidgets(
      "shows the snapshot in the first frame, over the home app's page, until it is shown",
      (tester) async {
        final image = await tester.runAsync(() async {
          final codec = await ui.instantiateImageCodec(await _png());
          return (await codec.getNextFrame()).image;
        });
        final c = cover(image: image);
        var painted = 0;
        await tester.pumpWidget(
          MaterialApp(
            home: HomeRoute(
              host: host,
              clock: clock,
              snapshot: c,
              onSnapshotPainted: () => painted++,
              app: (context, aid, hooks) => Text('page $aid'),
              fallback: (_, _) => const Text('built-in'),
            ),
          ),
        );
        expect(find.byKey(const ValueKey('home-snapshot')), findsOneWidget);
        final raw = tester.widget<RawImage>(find.byType(RawImage));
        expect(raw.image, same(image));
        expect(painted, 1);
        expect(find.text('built-in'), findsNothing);

        await shown(tester);
        expect(find.byKey(const ValueKey('home-snapshot')), findsNothing);
        expect(find.text('page $home'), findsOneWidget);
        expect(c.covering, isFalse);
        image!.dispose();
      },
    );

    testWidgets('a tap on a recorded tile opens that app once it is shown', (
      tester,
    ) async {
      await tester.runAsync(host.load);
      final c = cover();
      await pump(tester, c);
      await tester.tapAt(const Offset(150, 50));
      await tester.tapAt(const Offset(50, 50));
      expect(actions.opened, isEmpty);
      await shown(tester);
      // The first tap is the one kept.
      expect(actions.opened, [photos]);
      // A later shown (a reload) opens nothing more.
      await shown(tester);
      expect(actions.opened, [photos]);
    });

    testWidgets('a tap outside every tile opens nothing', (tester) async {
      await tester.runAsync(host.load);
      final c = cover();
      await pump(tester, c);
      await tester.tapAt(const Offset(400, 400));
      await shown(tester);
      expect(actions.opened, isEmpty);
      expect(c.covering, isFalse);
    });

    testWidgets("a tap on the tile of an app deleted since opens nothing", (
      tester,
    ) async {
      await tester.runAsync(() async {
        await api.deleteArtifact(notes);
        await host.load();
      });
      final c = cover();
      await pump(tester, c);
      await tester.tapAt(const Offset(50, 50));
      await shown(tester);
      expect(actions.opened, isEmpty);
    });

    testWidgets(
      'the built-in home screen coming instead drops the snapshot and its tap',
      (tester) async {
        await tester.runAsync(host.load);
        final c = cover();
        await pump(tester, c);
        await tester.tapAt(const Offset(50, 50));
        clock.advance(const Duration(seconds: 10));
        await tester.pump();
        expect(find.text('built-in'), findsOneWidget);
        expect(find.byKey(const ValueKey('home-snapshot')), findsNothing);
        await shown(tester);
        expect(actions.opened, isEmpty);
      },
    );

    testWidgets('a snapshot of another home app is not shown', (tester) async {
      final c = cover(aid: notes);
      await pump(tester, c);
      expect(find.byKey(const ValueKey('home-snapshot')), findsNothing);
      expect(find.text('page $home'), findsOneWidget);
    });
  });

  group("the home app's tile reports", () {
    late FakeClaxApi api;
    late String home;
    late String notes;
    late HomeHost host;

    setUp(() {
      api = FakeClaxApi();
      home = api.seedArtifact('Home', _page, capabilities: {'home': {}});
      notes = api.seedArtifact('Notes', _page);
      host = HomeHost(
        api: api,
        stream: api.stream(),
        closeStream: true,
        makes: MakeService((r, c, s) => Completer<String>().future),
        builds: null,
        settings: HomeSettings(homeAppId: home),
        actions: _Actions(),
      );
    });

    tearDown(() => host.dispose());

    test(
      'are kept and passed on; malformed ones and other apps are refused',
      () async {
        final heard = <List<HomeTile>>[];
        host.tileReports.listen(heard.add);
        expect(host.tiles, isNull);
        await host.call(home, 'tiles', {
          'tiles': [
            {
              'id': notes,
              'rect': {'x': 16, 'y': 80.5, 'w': 88, 'h': 104},
            },
          ],
        });
        expect(host.tiles, [
          HomeTile(notes, const Rect.fromLTWH(16, 80.5, 88, 104)),
        ]);
        expect(heard, hasLength(1));
        for (final bad in [
          'x',
          [
            {'id': notes},
          ],
          [
            {
              'id': '',
              'rect': {'x': 0, 'y': 0, 'w': 1, 'h': 1},
            },
          ],
          List.filled(HomeHost.maxTiles + 1, {
            'id': notes,
            'rect': {'x': 0, 'y': 0, 'w': 1, 'h': 1},
          }),
        ]) {
          await expectLater(
            host.call(home, 'tiles', {'tiles': bad}),
            throwsA(isA<HomeBadRequest>()),
          );
        }
        await expectLater(
          host.call(notes, 'tiles', {'tiles': const []}),
          throwsA(isA<HomeRefused>()),
        );
        expect(heard, hasLength(1));
      },
    );
  });
}
