import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:ui' show Rect;

import 'package:clax_app/agent/plan.dart';
import 'package:clax_app/agent/worker.dart';
import 'package:clax_app/bundle/installer.dart';
import 'package:clax_app/bundle/lineage_store.dart';
import 'package:clax_app/clax/api.dart';
import 'package:clax_app/clax/fake_api.dart';
import 'package:clax_app/home/home_host.dart';
import 'package:clax_app/home/home_install.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/ui/app_icon.dart';
import 'package:clax_app/ui/build_plan.dart';
import 'package:clax_app/ui/make_service.dart';
import 'dart:typed_data';
import 'package:flutter_test/flutter_test.dart';

import '../support/paths.dart';

const _page = {'index.html': '<!doctype html><title>x</title>'};

/// A fake whose stamps increase, so later artifacts are newer.
FakeClaxApi _api() {
  var t = 0;
  return FakeClaxApi(
    clock: () => '2026-10-12T00:00:${(t++).toString().padLeft(2, '0')}Z',
  );
}

/// A fake whose app list, once taken, answers only when [gate] completes.
class _GatedApi extends FakeClaxApi {
  _GatedApi() : super(clock: _api().clock);

  final gate = Completer<void>();

  @override
  Future<List<ArtifactSummary>> listArtifacts() async {
    final listed = await super.listArtifacts();
    await gate.future;
    return listed;
  }
}

/// A fake whose created apps are announced, and fetched by whoever
/// follows the gallery, before the create answers: the daemon's event can
/// overtake its response.
class _EventFirstApi extends FakeClaxApi {
  _EventFirstApi() : super(clock: _api().clock);

  @override
  Future<PublishResult> createArtifact(PublishRequest r) async {
    final made = await super.createArtifact(r);
    emit('version', {'artifact_id': made.artifact.id, 'n': 1});
    await pumpEventQueue();
    return made;
  }
}

/// Home settings that run [onSave] in place of writing a file.
class _SavedSettings extends HomeSettings {
  _SavedSettings(this.onSave);

  final void Function() onSave;

  @override
  Future<void> save() async => onSave();
}

/// The shipped home bundle and its release record, from the asset files.
class _Shipped implements ShippedHome {
  @override
  Future<Uint8List> bundle() async =>
      File(appPath(AssetShippedHome.bundlePath)).readAsBytesSync();

  @override
  Future<HomeRelease> release() async => HomeRelease.parse(
    File(appPath(AssetShippedHome.releasePath)).readAsStringSync(),
  );
}

class _Actions implements HomeActions {
  final calls = <(String, List<Object?>)>[];

  /// What the next `menu` resolves with.
  String? menuAnswer;
  @override
  void open(String id) => calls.add(('open', [id]));
  @override
  void openMake(String id) => calls.add(('openMake', [id]));
  @override
  void newApp() => calls.add(('newApp', []));
  @override
  Future<String?> menu(
    String id,
    Rect anchor,
    List<HomeMenuItem> items,
  ) async {
    calls.add(('menu', [id, anchor, items]));
    return menuAnswer;
  }

  @override
  void openInbox() => calls.add(('openInbox', []));
  @override
  void openSettings() => calls.add(('openSettings', []));
  @override
  void importApp() => calls.add(('importApp', []));
}

class _Control implements BuildControl {
  final _c = StreamController<BuildState>.broadcast(sync: true);
  final _latest = <String, BuildState>{};
  void push(BuildState s) {
    _latest[s.appId] = s;
    _c.add(s);
  }

  @override
  Stream<BuildState> get builds => _c.stream;
  @override
  BuildState? buildOf(String appId) => _latest[appId];
  @override
  Future<void> stopBuild(String appId) async {}
  @override
  Future<void> resumeBuild(String appId) async {}
}

void main() {
  late FakeClaxApi api;
  late _Actions actions;
  late HomeSettings settings;
  late MakeService makes;
  late PlanStore plans;
  late _Control control;
  late String home;
  late HomeHost host;

  HomeHost make() => HomeHost(
    api: api,
    stream: api.stream(),
    closeStream: true,
    makes: makes,
    builds: BuildLink(plans: plans, control: control),
    settings: settings,
    actions: actions,
  );

  setUp(() async {
    api = _api();
    actions = _Actions();
    makes = MakeService(
      (r, cancel, report) => r.startsWith('boom')
          ? Future.error(StateError('no key'))
          : Completer<String>().future,
    );
    plans = PlanStore();
    control = _Control();
    home = api.seedArtifact('Home', _page);
    settings = HomeSettings(homeAppId: home);
    host = make();
    await pumpEventQueue();
  });

  tearDown(() => host.dispose());

  test('apps() answers the home app with the gallery newest first, itself left out', () async {
    final old = api.seedArtifact('Old runs', _page);
    final fresh = api.seedArtifact('Groceries', _page);
    await host.load();
    final apps = await host.call(home, 'apps', {}) as List;
    expect([for (final a in apps) (a as Map)['id']], [fresh, old]);
    final first = apps.first as Map;
    expect(first['title'], 'Groceries');
    expect(first['build'], isNull);
    expect(first['icon'], {'monogram': 'G', 'hue': isA<int>()});
    expect(DateTime.tryParse(first['updatedAt'] as String), isNotNull);
    expect(host.state.apps.map((a) => a.id), [fresh, old]);
  });

  test('layout is the home app\'s arrangement, loaded again as the list changes', () async {
    final a = api.seedArtifact('A', _page);
    final b = api.seedArtifact('B', _page);
    await api.batchDocs(home, [
      DocWrite(
        op: 'set',
        collection: 'layout',
        docId: 'main',
        data: {
          'starred': [a],
          'order': [b],
        },
      ),
    ]);
    await host.load();
    final layout = host.layout!;
    expect(identical(host.layout, layout), isTrue);
    await pumpEventQueue();
    expect(layout.readable, isTrue);
    expect(layout.sections.starred, [a]);
    expect(layout.sections.order, [b]);

    final c = api.seedArtifact('C', _page);
    await host.load();
    await pumpEventQueue();
    expect(layout.sections.order, [c, b]);

    settings.homeAppId = null;
    expect(host.layout, isNull);
  });

  test(
    'another app is refused every call and never sees it available',
    () async {
      final other = api.seedArtifact('Mimic', _page);
      expect(host.availableTo(other), isFalse);
      expect(host.availableTo(home), isTrue);
      for (final m in [
        'apps',
        'makes',
        'unread',
        'needsKey',
        'open',
        'newApp',
      ]) {
        await expectLater(
          host.call(other, m, {'id': home}),
          throwsA(isA<HomeRefused>()),
        );
      }
      // The home app's own calls first change nothing for the next caller.
      await host.call(home, 'apps', {});
      expect(host.availableTo(other), isFalse);
      await expectLater(
        host.call(other, 'apps', {}),
        throwsA(isA<HomeRefused>()),
      );
      expect(actions.calls, isEmpty);
    },
  );

  test("apps() gives a listed app's icon.svg as a {url} face once read, "
      'with no tile built; a repeated list reads it no more', () async {
    const icon =
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">'
        '<rect width="1024" height="1024" fill="#f80"/></svg>';
    final runs = api.seedArtifact('Runs', {..._page, 'icon.svg': icon});
    final plain = api.seedArtifact('Plain', _page);
    host.dispose();
    final icons = AppIcons(api);
    host = HomeHost(
      api: api,
      stream: api.stream(),
      closeStream: true,
      makes: makes,
      builds: BuildLink(plans: plans, control: control),
      settings: settings,
      actions: actions,
      icons: icons,
    );
    final told = <HomeEvent>[];
    host.events.listen(told.add);
    await pumpEventQueue();

    Map<String, Object?> iconOf(List apps, String id) =>
        (apps.firstWhere((a) => (a as Map)['id'] == id) as Map)['icon']
            as Map<String, Object?>;
    final apps = await host.call(home, 'apps', {}) as List;
    final url = iconOf(apps, runs)['url'] as String;
    expect(url, startsWith('data:image/svg+xml;base64,'));
    expect(utf8.decode(base64.decode(url.split(',').last)), contains('#f80'));
    expect(iconOf(apps, plain), {'monogram': 'P', 'hue': isA<int>()});
    // The page heard of the icon without asking again.
    expect(iconOf(told.lastWhere((e) => e.name == 'apps').data as List, runs), {
      'url': url,
    });

    final reads = api.callNames.where((n) => n == 'readFile').length;
    expect(reads, 2, reason: "each app's live version read once");
    await host.load();
    await host.load();
    await pumpEventQueue();
    expect(api.callNames.where((n) => n == 'readFile').length, reads);
  });

  test('without a recorded home app every caller is refused', () async {
    settings.homeAppId = null;
    expect(host.availableTo(home), isFalse);
    await expectLater(host.call(home, 'apps', {}), throwsA(isA<HomeRefused>()));
  });

  test('a gallery version refetches that app and notifies once; a delete removes it', () async {
    final aid = api.seedArtifact('Runs', _page);
    await host.load();
    final fresh = api.seedArtifact('Fresh', _page);
    var heard = 0;
    host.addListener(() => heard++);
    final told = <HomeEvent>[];
    host.events.listen(told.add);
    api.emit('version', {'artifact_id': fresh, 'n': 1});
    await pumpEventQueue();
    expect(heard, 1);
    expect(host.state.apps.map((a) => a.id), [fresh, aid]);
    expect(told.map((e) => e.name), contains('apps'));
    api.emit('artifact_deleted', {'artifact_id': aid});
    await pumpEventQueue();
    expect(host.state.apps.map((a) => a.id), [fresh]);
    expect([for (final a in told.last.data as List) (a as Map)['id']], [fresh]);
  });

  test('forget drops a deleted app at once, and its stream delete then '
      'changes nothing', () async {
    final aid = api.seedArtifact('Runs', _page);
    final kept = api.seedArtifact('Kept', _page);
    await host.load();
    var heard = 0;
    host.addListener(() => heard++);
    host.forget(aid);
    expect(host.state.apps.map((a) => a.id), [kept]);
    expect(heard, 1);
    api.emit('artifact_deleted', {'artifact_id': aid});
    await pumpEventQueue();
    expect(host.state.apps.map((a) => a.id), [kept]);
    expect(heard, 1);
  });

  test('gallery events before the first list answers wait for it: apps() '
      'and the apps event never carry a partial list', () async {
    final gated = _GatedApi();
    final gatedHome = gated.seedArtifact('Home', _page);
    final runs = gated.seedArtifact('Runs', _page);
    final plain = gated.seedArtifact('Plain', _page);
    final gone = gated.seedArtifact('Gone', _page);
    final early = HomeHost(
      api: gated,
      stream: gated.stream(),
      closeStream: true,
      makes: makes,
      builds: BuildLink(plans: plans, control: control),
      settings: HomeSettings(homeAppId: gatedHome),
      actions: actions,
    );
    addTearDown(early.dispose);
    final told = <HomeEvent>[];
    early.events.listen(told.add);
    await pumpEventQueue();

    // The list was taken; a new app and a delete arrive before it answers.
    final fresh = gated.seedArtifact('Fresh', _page);
    gated.emit('version', {'artifact_id': fresh, 'n': 1});
    gated.emit('artifact_deleted', {'artifact_id': gone});
    await pumpEventQueue();
    expect(await early.call(gatedHome, 'apps', {}), isEmpty);
    expect(early.summaries, isNull);
    for (final e in told.where((e) => e.name == 'apps')) {
      expect(e.data, isEmpty);
    }

    gated.gate.complete();
    await pumpEventQueue();
    final apps = await early.call(gatedHome, 'apps', {}) as List;
    expect([for (final a in apps) (a as Map)['id']], [fresh, plain, runs]);
    expect(
      [for (final a in told.lastWhere((e) => e.name == 'apps').data as List)
        (a as Map)['id']],
      [fresh, plain, runs],
    );
  });

  test('an app a home install makes is never listed, even before '
      'homeAppId is saved', () async {
    final first = _EventFirstApi();
    await first.registerAppSession(installId: 'i-home', pid: 1);
    final runs = first.seedArtifact('Runs', _page);
    final tmp = Directory.systemTemp.createTempSync('home-first-run');
    addTearDown(() => tmp.deleteSync(recursive: true));
    List<String>? atSave;
    late HomeHost fresh;
    final s = _SavedSettings(
      () => atSave = [for (final a in fresh.summaries ?? const []) a.id],
    );
    fresh = HomeHost(
      api: first,
      stream: first.stream(),
      closeStream: true,
      makes: makes,
      builds: null,
      settings: s,
      actions: actions,
    );
    addTearDown(fresh.dispose);
    await pumpEventQueue();
    expect(fresh.summaries?.map((a) => a.id), [runs]);
    final heard = <List<String>>[];
    fresh.addListener(
      () => heard.add([for (final a in fresh.summaries ?? const []) a.id]),
    );

    final installer = Installer(
      api: first,
      store: LineageStore(Directory('${tmp.path}/support')),
      key: homeInstallKey(),
    );
    final aid = await ensureHomeApp(
      s,
      installer,
      first,
      shipped: _Shipped(),
      installing: fresh.installing,
    );
    await pumpEventQueue();

    expect(atSave, [runs], reason: 'left out before homeAppId was saved');
    for (final ids in heard) {
      expect(ids, isNot(contains(aid)));
    }
    expect(s.homeAppId, aid);
    expect(fresh.summaries?.map((a) => a.id), [runs]);
    await fresh.load();
    expect(fresh.summaries?.map((a) => a.id), [runs]);
  });

  test('a make that changes while nobody listens reads as it stands', () async {
    makes.dismiss(makes.add('Something else'));
    final s = makes.start('boom: a tracker for my runs');
    expect(host.state.makes.single.state, 'making');
    await pumpEventQueue();
    final m = host.state.makes.single;
    expect(m.state, 'failed');
    expect(host.sessionOf(m.id), same(s));
    final answered = await host.call(home, 'makes', {}) as List;
    expect(answered.single, {
      'id': m.id,
      'request': 'boom: a tracker for my runs',
      'state': 'failed',
      'progress': null,
    });
  });

  test("a going build shows its plan's progress", () async {
    final aid = api.seedArtifact('Runs', _page);
    await host.load();
    await plans.set(aid, [
      const PlanFeature(id: 'a', title: 'A', status: FeatureStatus.done),
      const PlanFeature(id: 'b', title: 'B'),
      const PlanFeature(id: 'c', title: 'C'),
    ]);
    control.push(BuildState(aid, BuildPhase.running));
    await pumpEventQueue();
    expect(host.buildOf(aid), (done: 1, total: 3));
    final apps = await host.call(home, 'apps', {}) as List;
    expect((apps.single as Map)['build'], {'done': 1, 'total': 3});
  });

  test('unread follows the inbox', () async {
    final aid = api.seedArtifact('Runs', _page);
    api.pushInbox('reply', artifactId: aid, body: 'Done');
    await pumpEventQueue();
    expect(await host.call(home, 'unread', {}), 1);
  });

  test('chrome() answers what the app set, and a change is one chrome event', () async {
    expect(await host.call(home, 'chrome', {}), {
      'platform': 'android',
      'form': 'phone',
      'toolbar': false,
      'pointer': 'touch',
    });
    final told = <HomeEvent>[];
    host.events.listen(told.add);
    await pumpEventQueue();
    told.clear();
    const mouse = (
      platform: 'android',
      form: 'wide',
      toolbar: false,
      pointer: 'mouse',
    );
    host.setChrome(mouse);
    host.setChrome(mouse);
    await pumpEventQueue();
    final wide = {
      'platform': 'android',
      'form': 'wide',
      'toolbar': false,
      'pointer': 'mouse',
    };
    expect(
      told.where((e) => e.name == 'chrome').map((e) => e.data),
      [wide],
    );
    expect(await host.call(home, 'chrome', {}), wide);
    expect(host.latest.map((e) => e.name), contains('chrome'));
  });

  test('while a holder holds the window\'s toolbar, chrome() answers toolbar '
      'true and form wide; the last release gives back what the app set', () async {
    final told = <HomeEvent>[];
    host.events.listen(told.add);
    await pumpEventQueue();
    told.clear();
    host.setChrome((
      platform: 'macos',
      form: 'phone',
      toolbar: false,
      pointer: 'mouse',
    ));
    final a = Object(), b = Object();
    host.holdToolbar(a, true);
    host.holdToolbar(b, true);
    host.holdToolbar(a, true);
    await pumpEventQueue();
    const held = {
      'platform': 'macos',
      'form': 'wide',
      'toolbar': true,
      'pointer': 'mouse',
      'query': '',
    };
    expect(await host.call(home, 'chrome', {}), held);
    expect(host.chrome.toolbar, isTrue);
    host.holdToolbar(a, false);
    await pumpEventQueue();
    expect(await host.call(home, 'chrome', {}), held);
    // The app's own reports keep their toolbar: false under the hold.
    host.setChrome((
      platform: 'macos',
      form: 'wide',
      toolbar: false,
      pointer: 'mouse',
    ));
    expect(host.chrome.toolbar, isTrue);
    host.holdToolbar(b, false);
    await pumpEventQueue();
    final free = {
      'platform': 'macos',
      'form': 'wide',
      'toolbar': false,
      'pointer': 'mouse',
    };
    expect(await host.call(home, 'chrome', {}), free);
    expect(
      told.where((e) => e.name == 'chrome').map((e) => e.data),
      [
        {...free, 'form': 'phone'},
        held,
        free,
      ],
    );
  });

  test('while the toolbar is held its search text is chrome()\'s query, one '
      'event per change; the last release clears it', () async {
    final told = <HomeEvent>[];
    host.events.listen(told.add);
    await pumpEventQueue();
    told.clear();
    // Without the toolbar there is no query to set.
    host.setQuery('gro');
    expect(host.query, '');
    expect(await host.call(home, 'chrome', {}), isNot(contains('query')));
    final a = Object();
    host.holdToolbar(a, true);
    host.setQuery('gro');
    host.setQuery('gro');
    await pumpEventQueue();
    expect(host.query, 'gro');
    expect(await host.call(home, 'chrome', {}), containsPair('query', 'gro'));
    host.holdToolbar(a, false);
    await pumpEventQueue();
    expect(host.query, '');
    expect(
      [
        for (final e in told)
          if (e.name == 'chrome') (e.data! as Map)['query'],
      ],
      ['', 'gro', null],
    );
    // Held again, the search starts empty.
    host.holdToolbar(a, true);
    expect(await host.call(home, 'chrome', {}), containsPair('query', ''));
  });

  test('refresh lists the apps and counts the unread again', () async {
    await host.load();
    final aid = api.seedArtifact('Runs', _page);
    expect(host.state.apps.map((a) => a.id), isNot(contains(aid)));
    api.calls.clear();
    await host.refresh();
    expect(host.state.apps.map((a) => a.id), contains(aid));
    expect(api.callNames, containsAll(['listArtifacts', 'inboxSummary']));
  });

  test('open, newApp and menu call the matching action', () async {
    final aid = api.seedArtifact('Runs', _page);
    await host.load();
    await host.call(home, 'open', {'id': aid});
    await host.call(home, 'newApp', {});
    await host.call(home, 'menu', {
      'id': aid,
      'rect': {'x': 1, 'y': 2, 'w': 3, 'h': 4},
    });
    await host.call(home, 'openInbox', {});
    expect(actions.calls.map((c) => [c.$1, ...c.$2]), [
      ['open', aid],
      ['newApp'],
      ['menu', aid, const Rect.fromLTWH(1, 2, 3, 4), const <HomeMenuItem>[]],
      ['openInbox'],
    ]);
  });

  test("menu passes the page's items and resolves the picked item's ID", () async {
    final aid = api.seedArtifact('Runs', _page);
    await host.load();
    actions.menuAnswer = 'star';
    final picked = await host.call(home, 'menu', {
      'id': aid,
      'rect': {'x': 1, 'y': 2, 'w': 3, 'h': 4},
      'items': [
        {'id': 'star', 'label': ' Star '},
        {'id': 'rm', 'label': 'Remove from group'},
      ],
    });
    expect(picked, 'star');
    expect(actions.calls.single.$2[2], [
      (id: 'star', label: 'Star'),
      (id: 'rm', label: 'Remove from group'),
    ]);
    actions.menuAnswer = null;
    expect(
      await host.call(home, 'menu', {
        'id': aid,
        'rect': {'x': 0, 'y': 0, 'w': 1, 'h': 1},
      }),
      isNull,
    );
  });

  test('menu takes a label of 32 characters', () async {
    final aid = api.seedArtifact('Runs', _page);
    await host.load();
    await host.call(home, 'menu', {
      'id': aid,
      'rect': {'x': 0, 'y': 0, 'w': 1, 'h': 1},
      'items': [
        {'id': 'a', 'label': 'é' * 32},
      ],
    });
    expect(actions.calls.single.$2[2], [(id: 'a', label: 'é' * 32)]);
  });

  test('menu refuses bad items as a bad request and shows nothing', () async {
    final aid = api.seedArtifact('Runs', _page);
    await host.load();
    final rect = {'x': 0, 'y': 0, 'w': 1, 'h': 1};
    for (final items in <Object?>[
      [
        for (var i = 0; i < 5; i++) {'id': 'i$i', 'label': 'x'},
      ],
      [
        {'id': 'a', 'label': 'x' * 33},
      ],
      [
        {'id': 'a', 'label': '  '},
      ],
      [
        {'id': '', 'label': 'Star'},
      ],
      [
        {'id': 'a' * 65, 'label': 'Star'},
      ],
      [
        {'id': 'a', 'label': 'Star'},
        {'id': 'a', 'label': 'Unstar'},
      ],
      [
        {'id': 'a', 'label': 'Line\nbreak'},
      ],
      [
        {'id': 3, 'label': 'Star'},
      ],
      'star',
    ]) {
      await expectLater(
        host.call(home, 'menu', {'id': aid, 'rect': rect, 'items': items}),
        throwsA(isA<HomeBadRequest>()),
        reason: '$items',
      );
    }
    expect(actions.calls, isEmpty);
  });

  test('while actions are held, menu resolves null without showing', () async {
    final aid = api.seedArtifact('Runs', _page);
    await host.load();
    host.holdActions = () => true;
    actions.menuAnswer = 'star';
    expect(
      await host.call(home, 'menu', {
        'id': aid,
        'rect': {'x': 0, 'y': 0, 'w': 1, 'h': 1},
        'items': [
          {'id': 'star', 'label': 'Star'},
        ],
      }),
      isNull,
    );
    expect(actions.calls, isEmpty);
  });

  test(
    'a call naming no listed app, or a bad method, is a bad request',
    () async {
      await host.load();
      for (final (m, a) in [
        ('open', {'id': home}),
        ('open', <String, Object?>{}),
        ('menu', {'id': 'nope', 'rect': {}}),
        ('openMake', {'id': 'm99'}),
        ('reboot', <String, Object?>{}),
      ]) {
        await expectLater(
          host.call(home, m, a),
          throwsA(isA<HomeBadRequest>()),
          reason: m,
        );
      }
      expect(actions.calls, isEmpty);
    },
  );

  test('home.json keeps the home app across loads', () async {
    final dir = await Directory.systemTemp.createTemp('home-settings');
    addTearDown(() => dir.delete(recursive: true));
    final s = await HomeSettings.load(dir);
    expect(s.homeAppId, isNull);
    s
      ..homeAppId = 'abc123'
      ..keptRelease = '1.2.0';
    await s.save();
    final again = await HomeSettings.load(dir);
    expect(again.homeAppId, 'abc123');
    expect(again.builtin, isFalse);
    expect(again.keptRelease, '1.2.0');
    await File('${dir.path}/home.json').writeAsString('{torn');
    expect((await HomeSettings.load(dir)).homeAppId, isNull);
  });
}
