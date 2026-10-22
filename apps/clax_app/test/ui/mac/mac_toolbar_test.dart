import 'dart:io';

import 'package:clax_app/ui/mac/mac_toolbar.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  late List<MethodCall> calls;

  setUp(() {
    debugDefaultTargetPlatformOverride = TargetPlatform.macOS;
    MacToolbar.reset();
    calls = [];
    messenger.setMockMethodCallHandler(MacToolbar.channel, (call) async {
      calls.add(call);
      return call.method == 'share' ? true : null;
    });
  });

  tearDown(() {
    messenger.setMockMethodCallHandler(MacToolbar.channel, null);
    debugDefaultTargetPlatformOverride = null;
  });

  const back = ToolbarItemSpec(
    'back',
    symbol: 'chevron.left',
    label: 'Back',
    group: 'navigation',
  );
  const threads = ToolbarItemSpec(
    'threads',
    symbol: 'list.bullet.rectangle',
    label: 'Threads',
    badge: 3,
  );
  final owner = Object();

  test('setItems sends each spec, once until they change', () async {
    MacToolbar.setItems(owner, const [back, threads], view: 1);
    MacToolbar.setItems(owner, const [back, threads], view: 1);
    await pumpEventQueue();
    expect(calls, hasLength(1));
    expect(calls.single.method, 'setItems');
    expect(calls.single.arguments, {
      'view': 1,
      'items': [
        {
          'id': 'back',
          'symbol': 'chevron.left',
          'label': 'Back',
          'enabled': true,
          'on': false,
          'group': 'navigation',
        },
        {
          'id': 'threads',
          'symbol': 'list.bullet.rectangle',
          'label': 'Threads',
          'enabled': true,
          'on': false,
          'badge': 3,
        },
      ],
    });

    MacToolbar.setItems(owner, const [
      back,
      ToolbarItemSpec(
        'comment',
        symbol: 'text.bubble',
        label: 'Comment',
        on: true,
        tint: Color(0xFFE0532F),
        badge: 0,
      ),
    ], view: 1);
    await pumpEventQueue();
    expect(calls, hasLength(2));
    final items = (calls.last.arguments as Map)['items'] as List;
    expect(items.last, {
      'id': 'comment',
      'symbol': 'text.bubble',
      'label': 'Comment',
      'enabled': true,
      'on': true,
      'tint': '#e0532f',
    });
  });

  test('only the owner that declared the items clears them', () async {
    final next = Object();
    MacToolbar.setItems(owner, const [back], view: 1);
    MacToolbar.setItems(next, const [threads], view: 1);
    // The screen that went away clears nothing of the next one's.
    MacToolbar.clear(owner, view: 1);
    await pumpEventQueue();
    expect(calls, hasLength(2));
    expect(MacToolbar.owns(next, view: 1), isTrue);
    MacToolbar.clear(next, view: 1);
    await pumpEventQueue();
    expect(calls.last.arguments, {'view': 1, 'items': <Object>[]});
    expect(MacToolbar.owns(next, view: 1), isFalse);
  });

  test('each window keeps its own items', () async {
    MacToolbar.setItems(owner, const [back], view: 1);
    MacToolbar.setItems(owner, const [back], view: 2);
    await pumpEventQueue();
    expect([for (final c in calls) (c.arguments as Map)['view']], [1, 2]);
  });

  test('a window\'s leading items go before what its owner declares, and '
      'only while it declares any', () async {
    const sidebar = ToolbarItemSpec(
      'sidebar',
      symbol: 'sidebar.left',
      label: 'Sidebar',
    );
    List<Object?> ids() => [
      for (final i in (calls.last.arguments as Map)['items'] as List)
        (i as Map)['id'],
    ];
    MacToolbar.setLeading(const [sidebar], view: 1);
    await pumpEventQueue();
    expect(calls, isEmpty);
    MacToolbar.setItems(owner, const [back, threads], view: 1);
    await pumpEventQueue();
    expect(ids(), ['sidebar', 'back', 'threads']);
    MacToolbar.setLeading(const [
      ToolbarItemSpec('sidebar', symbol: 'sidebar.left', label: 'Sidebar', on: true),
    ], view: 1);
    await pumpEventQueue();
    expect(calls, hasLength(2));
    expect(((calls.last.arguments as Map)['items'] as List).first, {
      'id': 'sidebar',
      'symbol': 'sidebar.left',
      'label': 'Sidebar',
      'enabled': true,
      'on': true,
    });
    MacToolbar.clear(owner, view: 1);
    await pumpEventQueue();
    expect(ids(), isEmpty);
    MacToolbar.setLeading(const [], view: 1);
    MacToolbar.setItems(owner, const [back], view: 1);
    await pumpEventQueue();
    expect(ids(), ['back']);
  });

  test('leading items over a sidebar send its width, so what follows them '
      'starts past its edge', () async {
    const sidebar = ToolbarItemSpec(
      'sidebar',
      symbol: 'sidebar.left',
      label: 'Hide Sidebar',
    );
    MacToolbar.setLeading(const [sidebar], view: 1, edge: 220);
    MacToolbar.setItems(owner, const [back], view: 1);
    await pumpEventQueue();
    expect(calls.last.arguments, containsPair('leading', 1));
    expect(calls.last.arguments, containsPair('edge', 220.0));
    // A new width is sent; the same is not.
    MacToolbar.setLeading(const [sidebar], view: 1, edge: 260);
    MacToolbar.setLeading(const [sidebar], view: 1, edge: 260);
    await pumpEventQueue();
    expect(calls, hasLength(2));
    expect(calls.last.arguments, containsPair('edge', 260.0));
    // A hidden sidebar has no edge.
    MacToolbar.setLeading(const [sidebar], view: 1);
    await pumpEventQueue();
    expect(calls, hasLength(3));
    expect(calls.last.arguments, isNot(contains('edge')));
    expect(calls.last.arguments, isNot(contains('leading')));
  });

  test('a click from the native side reaches clicks', () async {
    final clicks = <ToolbarClick>[];
    final sub = MacToolbar.clicks.listen(clicks.add);
    addTearDown(sub.cancel);
    Object? reply;
    await messenger.handlePlatformMessage(
      MacToolbar.channel.name,
      MacToolbar.channel.codec.encodeMethodCall(
        const MethodCall('clicked', {'view': 1, 'id': 'threads'}),
      ),
      (data) => reply = MacToolbar.channel.codec.decodeEnvelope(data!),
    );
    expect(clicks, [const ToolbarClick(1, 'threads')]);
    expect(reply, isTrue);
    await messenger.handlePlatformMessage(
      MacToolbar.channel.name,
      MacToolbar.channel.codec.encodeMethodCall(
        const MethodCall('clicked', {'id': 'threads'}),
      ),
      (data) => reply = MacToolbar.channel.codec.decodeEnvelope(data!),
    );
    expect(clicks, hasLength(1));
    expect(reply, isFalse);
  });

  test('a search item is sent as one, and its text and focus go both ways',
      () async {
    MacToolbar.setItems(owner, const [
      back,
      ToolbarItemSpec(
        'search',
        symbol: 'magnifyingglass',
        label: 'Search Apps',
        search: true,
      ),
    ], view: 1);
    await pumpEventQueue();
    final items = (calls.single.arguments as Map)['items'] as List;
    expect(items.last, {
      'id': 'search',
      'symbol': 'magnifyingglass',
      'label': 'Search Apps',
      'enabled': true,
      'on': false,
      'search': true,
    });

    final searches = <ToolbarSearch>[];
    final sub = MacToolbar.searches.listen(searches.add);
    addTearDown(sub.cancel);
    Object? reply;
    await messenger.handlePlatformMessage(
      MacToolbar.channel.name,
      MacToolbar.channel.codec.encodeMethodCall(
        const MethodCall('search', {'view': 1, 'id': 'search', 'text': 'gro'}),
      ),
      (data) => reply = MacToolbar.channel.codec.decodeEnvelope(data!),
    );
    expect(searches, [const ToolbarSearch(1, 'search', 'gro')]);
    expect(reply, isTrue);

    messenger.setMockMethodCallHandler(MacToolbar.channel, (call) async {
      calls.add(call);
      return call.method == 'focusSearch' &&
          (call.arguments as Map)['id'] == 'search';
    });
    expect(await MacToolbar.focusSearch('search', view: 1), isTrue);
    expect(calls.last.method, 'focusSearch');
    expect(calls.last.arguments, {'view': 1, 'id': 'search'});
    expect(await MacToolbar.focusSearch('other', view: 1), isFalse);
  });

  test('a choice is sent as selectable, and differs from the same item that '
      'is not', () async {
    const tab = ToolbarItemSpec(
      'general',
      symbol: 'gearshape',
      label: 'General',
      on: true,
      select: true,
    );
    MacToolbar.setItems(owner, const [tab], view: 2);
    await pumpEventQueue();
    expect((calls.single.arguments as Map)['items'], [
      {
        'id': 'general',
        'symbol': 'gearshape',
        'label': 'General',
        'enabled': true,
        'on': true,
        'select': true,
      },
    ]);
    expect(
      tab,
      isNot(
        const ToolbarItemSpec(
          'general',
          symbol: 'gearshape',
          label: 'General',
          on: true,
        ),
      ),
    );
  });

  test('ToolbarShareFiles writes the bundle and shares its path from the '
      'Share item', () async {
    final files = ToolbarShareFiles(view: 1);
    final bytes = Uint8List.fromList([1, 2, 3]);
    final chosen = await files.send(
      'Groceries.clax',
      bytes,
      origin: Rect.zero,
    );
    expect(chosen, isTrue);
    final call = calls.single;
    expect(call.method, 'share');
    final args = call.arguments as Map;
    expect(args['view'], 1);
    expect(args['item'], 'share');
    final path = args['path'] as String;
    expect(path, endsWith('/Groceries.clax'));
    expect(File(path).readAsBytesSync(), bytes);
    File(path).parent.deleteSync(recursive: true);
  });

  test('off the Mac nothing is sent', () async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    MacToolbar.setItems(owner, const [back], view: 1);
    expect(await MacToolbar.share('/x.clax', item: 'share', view: 1), isFalse);
    expect(await MacToolbar.focusSearch('search', view: 1), isFalse);
    await pumpEventQueue();
    expect(calls, isEmpty);
  });
}
