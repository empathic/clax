import 'dart:async';
import 'dart:convert';
import 'dart:ui' as ui;

import 'package:clax_app/ui/comment_composer.dart';
import 'package:clax_app/ui/host_protocol.dart';
import 'package:clax_app/ui/kit/text_field.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

/// A 1 x 1 PNG.
final _png = base64Decode(
  'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==',
);

const _element = Draft(
  key: 'p1',
  kind: 'element',
  label: '“Mon 5 km”',
  quote: 'Mon 5 km',
  text: '',
  clip: DraftClip.ready,
);

/// A plain [w] x [h] PNG.
Future<Uint8List> _plainPng(int w, int h) async {
  final rec = ui.PictureRecorder();
  Canvas(rec).drawRect(
    Rect.fromLTWH(0, 0, w.toDouble(), h.toDouble()),
    Paint()..color = const Color(0xff3366cc),
  );
  final image = await rec.endRecording().toImage(w, h);
  final data = await image.toByteData(format: ui.ImageByteFormat.png);
  image.dispose();
  return data!.buffer.asUint8List();
}

void main() {
  late TextEditingController text;
  late List<String> pressed;

  setUp(() {
    text = TextEditingController();
    pressed = [];
  });
  tearDown(() => text.dispose());

  Future<void> pump(
    WidgetTester tester,
    Draft draft, {
    bool withClip = true,
    Dictation dictation = const Dictation(),
    double width = 390,
    double scale = 1.0,
    bool plan = false,
    bool sending = false,
    Widget? appIcon,
  }) async {
    tester.view.devicePixelRatio = 3;
    tester.view.physicalSize = Size(width, 700) * 3;
    addTearDown(tester.view.reset);
    await tester.pumpWidget(
      MaterialApp(
        theme: claxTheme(Brightness.light, defaultTargetPlatform),
        home: MediaQuery.withClampedTextScaling(
          minScaleFactor: scale,
          maxScaleFactor: scale,
          child: Scaffold(
            body: Align(
              alignment: Alignment.bottomCenter,
              child: CommentComposer(
                draft: draft,
                clip: withClip && draft.clip == DraftClip.ready ? _png : null,
                controller: text,
                dictation: dictation,
                sending: sending,
                onMic: () => pressed.add('mic'),
                onSend: () => pressed.add('send'),
                onCancel: () => pressed.add('cancel'),
                onAddToPlan: plan ? () => pressed.add('plan') : null,
                appIcon: appIcon,
              ),
            ),
          ),
        ),
      ),
    );
    await tester.pump();
  }

  test("pngSize reads a PNG's header, and nothing else", () {
    expect(pngSize(_png), const Size(1, 1));
    expect(pngSize(Uint8List.fromList(List.filled(30, 0))), isNull);
  });

  String words(WidgetTester tester) =>
      tester.widget<Text>(find.byKey(const ValueKey('snippet-words'))).data!;

  testWidgets(
    "a picked element shows its screenshot beside its words, in the app's quote marks",
    (tester) async {
      await pump(tester, _element);
      expect(find.byKey(const ValueKey('snippet-clip')), findsOneWidget);
      expect(find.text('Your pick'), findsOneWidget);
      expect(words(tester), '“Mon 5 km”');
      final thumb = tester.getRect(find.byKey(const ValueKey('snippet-clip')));
      expect(thumb.width, lessThanOrEqualTo(96));
      expect(thumb.height, lessThanOrEqualTo(72));
    },
  );

  testWidgets("the quote's rule is people's red-orange, and the field is "
      "the kit's", (tester) async {
    await pump(tester, _element);
    final scheme = claxTheme(Brightness.light, defaultTargetPlatform)
        .colorScheme;
    final rule = find.ancestor(
      of: find.byKey(const ValueKey('snippet-words')),
      matching: find.byType(Container),
    );
    final box = tester.widget<Container>(rule.first).decoration!
        as BoxDecoration;
    expect((box.border! as BorderDirectional).start.color, scheme.tertiary);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('composer')),
        matching: find.byType(ClaxTextField),
      ),
      findsOneWidget,
    );
  });

  testWidgets('a composer the page opened says From this app, with the '
      "app's icon, never Your pick", (tester) async {
    await pump(
      tester,
      const Draft(
        key: 'g1',
        kind: 'custom',
        label: 'Rate this run',
        text: '',
        clip: DraftClip.none,
        byPage: true,
      ),
      appIcon: const SizedBox(key: ValueKey('the-app-icon')),
    );
    expect(find.text('From this app'), findsOneWidget);
    expect(find.text('Your pick'), findsNothing);
    expect(
      find.descendant(
        of: find.byKey(const ValueKey('snippet')),
        matching: find.byKey(const ValueKey('the-app-icon')),
      ),
      findsOneWidget,
    );
    expect(words(tester), 'Rate this run');
  });

  testWidgets('a screenshot still being taken has a placeholder', (
    tester,
  ) async {
    await pump(
      tester,
      const Draft(key: 'p1', label: '“Mon”', text: '', clip: DraftClip.pending),
    );
    expect(find.byKey(const ValueKey('snippet-pending')), findsOneWidget);
    expect(find.byKey(const ValueKey('snippet-clip')), findsNothing);
  });

  testWidgets('selected text shows more of the text it quotes', (tester) async {
    const quote =
        'Grow revenue in every region this year, and hire two more people '
        'for the support team before the summer';
    await pump(
      tester,
      const Draft(
        key: 'r1',
        kind: 'range',
        label: '“Grow revenue in every region this year, and hire two more…”',
        quote: quote,
        text: '',
        clip: DraftClip.ready,
      ),
    );
    expect(find.text('Selected text'), findsOneWidget);
    expect(words(tester), '“$quote”');
    expect(
      tester.widget<Text>(find.byKey(const ValueKey('snippet-words'))).maxLines,
      3,
    );
  });

  testWidgets("an area shows its screenshot and the shell's words for it", (
    tester,
  ) async {
    await pump(
      tester,
      const Draft(
        key: 'a1',
        kind: 'area',
        label: 'part of an image',
        text: '',
        clip: DraftClip.ready,
      ),
    );
    expect(find.text('Area'), findsOneWidget);
    expect(words(tester), 'part of an image');
    expect(find.byKey(const ValueKey('snippet-clip')), findsOneWidget);
  });

  testWidgets('the whole app is a small chip, with no screenshot', (
    tester,
  ) async {
    await pump(
      tester,
      const Draft(
        key: 'w1',
        kind: 'artifact',
        label: 'The whole app',
        text: '',
        clip: DraftClip.none,
      ),
    );
    expect(find.byKey(const ValueKey('snippet-whole-app')), findsOneWidget);
    expect(find.text('Whole app'), findsOneWidget);
    expect(find.byKey(const ValueKey('snippet')), findsNothing);
  });

  testWidgets('Send waits for text, then sends; Cancel and Add to plan', (
    tester,
  ) async {
    await pump(tester, _element, plan: true);
    ButtonStyleButton send() =>
        tester.widget<ButtonStyleButton>(find.byKey(const ValueKey('send')));
    expect(send().onPressed, isNull);
    await tester.enterText(
      find.byKey(const ValueKey('composer-field')),
      'bigger',
    );
    await tester.pump();
    expect(send().onPressed, isNotNull);
    await tester.tap(find.byKey(const ValueKey('send')));
    await tester.tap(find.byKey(const ValueKey('add-to-plan')));
    await tester.tap(find.byKey(const ValueKey('composer-cancel')));
    expect(pressed, ['send', 'plan', 'cancel']);
    expect(find.text('Send'), findsOneWidget);
  });

  testWidgets('while sending, Send and the field wait', (tester) async {
    text.text = 'bigger';
    await pump(tester, _element, sending: true);
    expect(find.text('Sending…'), findsOneWidget);
    expect(
      tester
          .widget<ButtonStyleButton>(find.byKey(const ValueKey('send')))
          .onPressed,
      isNull,
    );
    expect(
      tester
          .widget<TextField>(
            find.descendant(
              of: find.byKey(const ValueKey('composer-field')),
              matching: find.byType(TextField),
            ),
          )
          .enabled,
      isFalse,
    );
  });

  testWidgets('dictation: listening, and a mic that cannot run says why', (
    tester,
  ) async {
    await pump(tester, _element, dictation: const Dictation(listening: true));
    expect(find.text('Listening…'), findsOneWidget);
    expect(find.byIcon(Icons.graphic_eq), findsOneWidget);
    await tester.tap(find.byKey(const ValueKey('dictate')));
    expect(pressed, ['mic']);

    await pump(
      tester,
      _element,
      dictation: const Dictation(unavailable: 'Microphone access is off'),
    );
    expect(find.text('Microphone access is off'), findsOneWidget);
    expect(
      tester
          .widget<IconButton>(find.byKey(const ValueKey('dictate')))
          .onPressed,
      isNull,
    );
  });

  for (final width in [320.0, 390.0]) {
    for (final scale in [1.0, 1.3]) {
      testWidgets('fits $width dp at text scale $scale with Add to plan', (
        tester,
      ) async {
        text.text = 'Make the Monday card show the pace per kilometre';
        await pump(
          tester,
          const Draft(
            key: 'r1',
            kind: 'range',
            label: '“Grow revenue”',
            quote:
                'Grow revenue in every region this year, and hire two more '
                'people for the support team before the summer starts',
            text: '',
            clip: DraftClip.ready,
          ),
          width: width,
          scale: scale,
          plan: true,
          dictation: const Dictation(error: 'Dictation stopped: no speech'),
        );
        expect(tester.takeException(), isNull);
        final sheet = tester.getRect(find.byKey(const ValueKey('composer')));
        for (final k in ['send', 'composer-cancel', 'add-to-plan']) {
          final r = tester.getRect(find.byKey(ValueKey(k)));
          expect(
            sheet.contains(r.topLeft) &&
                sheet.contains(r.bottomRight - const Offset(0.1, 0.1)),
            isTrue,
            reason: k,
          );
        }
      });
    }
  }

  testWidgets(
    'a full-size screenshot is decoded at its thumbnail size, in device pixels',
    (tester) async {
      tester.view.devicePixelRatio = 3;
      addTearDown(tester.view.resetDevicePixelRatio);
      final png = (await tester.runAsync(() => _plainPng(1600, 800)))!;
      await tester.pumpWidget(
        MaterialApp(
          home: Center(child: ClipThumb(png, maxWidth: 96, maxHeight: 72)),
        ),
      );
      final provider = tester.widget<Image>(find.byType(Image)).image;
      expect(provider, isA<ResizeImage>());
      expect((provider as ResizeImage).width, 96 * 3);
      expect(provider.height, 72 * 3);
      expect(provider.policy, ResizeImagePolicy.fit);

      // Decode it, then read what the thumbnail holds.
      await tester.runAsync(() {
        final done = Completer<void>();
        provider
            .resolve(
              createLocalImageConfiguration(tester.element(find.byType(Image))),
            )
            .addListener(
              ImageStreamListener(
                (_, _) => done.isCompleted ? null : done.complete(),
                onError: (e, s) => done.completeError(e, s),
              ),
            );
        return done.future;
      });
      await tester.pump();
      final decoded = tester.widget<RawImage>(find.byType(RawImage)).image!;
      expect(decoded.width, 288);
      expect(decoded.height, 144);
      // It looks as it did: the whole screenshot, its proportions kept.
      expect(tester.getSize(find.byType(RawImage)), const Size(96, 48));
    },
  );
}
