// The kit's components rendered in every form and theme into
// CLAX_SHOTS_DIR (skipped without it): `kit-controls` is one page with
// every control in each of its states; `kit-surfaces-*` show a sheet, a
// dialog, a menu and each place of toast over one screen; `kit-mark` shows
// the mark in each state at 16, 24, 48 and 132, each caught at its widest.
import 'package:clax_app/ui/kit/kit.dart';
import 'package:clax_app/ui/layout.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/material.dart';

import 'rig.dart';

/// Every control in each of its states, in two columns from medium.
class _Controls extends StatelessWidget {
  const _Controls();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    Widget head(String text) => Padding(
      padding: const EdgeInsets.only(top: 16, bottom: 8),
      child: Text(text, style: theme.textTheme.titleSmall),
    );
    Widget row(List<Widget> children) => Wrap(
      spacing: 8,
      runSpacing: 8,
      crossAxisAlignment: WrapCrossAlignment.center,
      children: children,
    );
    void none() {}
    final buttons = [
      head('Buttons'),
      row([
        ClaxButton(
          label: 'Make it',
          onPressed: none,
          kind: ClaxButtonKind.primary,
        ),
        ClaxButton(
          label: 'Open thread',
          onPressed: none,
          kind: ClaxButtonKind.tonal,
        ),
        ClaxButton(label: 'Cancel', onPressed: none),
        ClaxButton(
          label: 'Delete',
          onPressed: none,
          kind: ClaxButtonKind.destructive,
        ),
      ]),
      const SizedBox(height: 8),
      row([
        ClaxButton(
          label: 'Save',
          onPressed: none,
          kind: ClaxButtonKind.primary,
          isDefault: true,
        ),
        ClaxButton(
          label: 'New app',
          onPressed: none,
          kind: ClaxButtonKind.tonal,
          icon: Icons.add,
        ),
        ClaxButton(label: 'Share', onPressed: none, icon: Icons.ios_share),
        const ClaxButton(
          label: 'Disabled',
          onPressed: null,
          kind: ClaxButtonKind.primary,
        ),
        const ClaxButton(label: 'Disabled', onPressed: null),
      ]),
      head('Fields'),
      ClaxTextField(
        label: 'API key',
        controller: TextEditingController(text: 'sk-ant-api03-shots'),
        obscure: true,
      ),
      const SizedBox(height: 12),
      const ClaxTextField(label: 'Name', hint: 'Groceries'),
      const SizedBox(height: 12),
      ClaxTextField(
        label: 'What should it do?',
        controller: TextEditingController(
          text: 'A shopping list my partner and I both edit, sorted by aisle.',
        ),
        maxLines: 3,
      ),
    ];
    final toggles = [
      head('Toggles'),
      ClaxSwitch(value: true, onChanged: (_) {}, label: 'Notifications'),
      ClaxSwitch(value: false, onChanged: (_) {}, label: 'Developer'),
      const ClaxSwitch(value: true, onChanged: null, label: 'Disabled'),
      ClaxCheckbox(
        value: true,
        onChanged: (_) {},
        label: 'Always for this app',
      ),
      ClaxCheckbox(value: false, onChanged: (_) {}, label: 'Include my data'),
      const ClaxCheckbox(value: false, onChanged: null, label: 'Disabled'),
      ClaxRadio<String>(
        value: 'precise',
        group: 'precise',
        onChanged: (_) {},
        label: 'Precise',
        detail: 'Your exact spot',
      ),
      ClaxRadio<String>(
        value: 'approximate',
        group: 'precise',
        onChanged: (_) {},
        label: 'Approximate',
        detail: 'Within a few kilometres',
      ),
      head('Badges'),
      Wrap(
        spacing: 24,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
        const ClaxBadge(count: 3, child: Icon(Icons.inbox_outlined)),
        const ClaxBadge(count: 120, child: Icon(Icons.inbox_outlined)),
        const ClaxBadge(count: 0, child: Icon(Icons.inbox_outlined)),
        const ClaxBadge(count: 7),
        const ClaxBadge(count: 100),
        ],
      ),
    ];
    final banners = [
      head('Banners'),
      ClaxBanner(
        icon: Icons.key_outlined,
        title: 'Add your API key',
        body: 'Clax needs it to make and change apps.',
        actions: [
          ClaxButton(
            label: 'Add key',
            onPressed: none,
            kind: ClaxButtonKind.tonal,
          ),
        ],
      ),
      const SizedBox(height: 12),
      ClaxBanner(
        icon: Icons.error_outline,
        iconColor: p.danger,
        title: 'Your home screen didn’t load',
        body: 'Clax is showing a simpler one for now.',
        actions: [
          ClaxButton(label: 'Details', onPressed: none),
          ClaxButton(
            label: 'Try again',
            onPressed: none,
            kind: ClaxButtonKind.primary,
            isDefault: true,
          ),
        ],
      ),
      head('Empty'),
      SizedBox(
        height: 260,
        child: ClaxEmpty(
          icon: Icons.inbox_outlined,
          title: 'All caught up.',
          body: 'Comments people send to Clax show up here.',
          actions: [ClaxButton(label: 'Back to Home', onPressed: none)],
          extra: Text(
            'Nothing waits here',
            style: ClaxType.of(ClaxRole.haiku, theme.platform)
                .copyWith(color: p.muted),
          ),
        ),
      ),
    ];
    return Scaffold(
      body: SafeArea(
        child: LayoutBuilder(
          builder: (context, box) {
            final gutter = ClaxBreakpoints.gutter(context);
            if (box.maxWidth < 840) {
              return ListView(
                padding: EdgeInsets.symmetric(horizontal: gutter),
                children: [...buttons, ...toggles, ...banners],
              );
            }
            Widget column(List<Widget> children) => Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: children,
              ),
            );
            return SingleChildScrollView(
              padding: EdgeInsets.symmetric(horizontal: gutter),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  column([...buttons, ...toggles]),
                  SizedBox(width: gutter * 2),
                  column(banners),
                ],
              ),
            );
          },
        ),
      ),
    );
  }
}

/// When each state is caught at its widest, in ms after it begins: the
/// breaths at their lean, sent as the dot reaches Clax's arc, landed at the
/// top of its nod, the rally as the dot leaves Clax's arc on its third hit.
const _peak = {
  ClaxMarkState.resting: 0,
  ClaxMarkState.working: 800,
  ClaxMarkState.sent: 150,
  ClaxMarkState.landed: 120,
  ClaxMarkState.asking: 800,
  ClaxMarkState.rally: 610,
};

/// The longest of [_peak]: every state is started this long before the render
/// less its own peak.
const _render = 800;

/// The mark's states, a row each at four sizes, then the mark in one colour
/// on `fg` and on `you`; each state starts when [started] lists it.
class _Marks extends StatelessWidget {
  const _Marks(this.started);
  final Set<ClaxMarkState> started;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final p = claxPalette(context);
    Widget row(String name, Widget Function(double size) mark) => Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.center,
        children: [
          SizedBox(
            width: 60,
            child: Text(
              name,
              style: theme.textTheme.bodySmall!.copyWith(color: p.muted),
            ),
          ),
          for (final (i, size) in const [16.0, 24.0, 48.0, 132.0].indexed) ...[
            if (i > 0) const SizedBox(width: 12),
            mark(size),
          ],
        ],
      ),
    );
    return Scaffold(
      body: SafeArea(
        child: ListView(
          padding: EdgeInsets.symmetric(
            horizontal: ClaxBreakpoints.gutter(context),
            vertical: 16,
          ),
          children: [
            for (final state in ClaxMarkState.values)
              row(
                state.name,
                (size) => ClaxMark(
                  size: size,
                  state: started.contains(state) ? state : ClaxMarkState.resting,
                ),
              ),
            row(
              'on fg',
              (size) => Container(
                padding: EdgeInsets.all(size / 8),
                decoration: BoxDecoration(
                  color: p.fg,
                  borderRadius: BorderRadius.circular(size),
                ),
                child: ClaxMark(size: size, monochrome: p.bg),
              ),
            ),
            row(
              'quiet',
              (size) => ClaxQuiet(
                quiet: true,
                child: ClaxMark(size: size, state: ClaxMarkState.asking),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

final _page = GlobalKey();
final _more = GlobalKey();

/// The screen the surfaces open over: a list of items in a scaffold, the
/// first row with a "more" button the menu anchors to.
class _Screen extends StatelessWidget {
  const _Screen();

  @override
  Widget build(BuildContext context) => ClaxScaffold(
    title: 'Groceries',
    subtitle: 'Shared with Sam',
    actions: [IconButton(onPressed: () {}, icon: const Icon(Icons.search))],
    body: Builder(
      key: _page,
      builder: (context) => ListView(
        children: [
          for (final (i, item) in const [
            'Oat milk',
            'Sourdough',
            'Lemons',
            'Coffee beans',
            'Spinach',
            'Eggs',
          ].indexed)
            ListTile(
              title: Text(item),
              subtitle: const Text('Aisle 4'),
              trailing: IconButton(
                key: i == 0 ? _more : null,
                onPressed: () {},
                icon: const Icon(Icons.more_horiz),
              ),
            ),
        ],
      ),
    ),
  );
}

/// Renders [surface] with [open] run on the screen's body.
void _surface(
  String surface,
  void Function(BuildContext context) open, {
  bool timers = false,
}) {
  shots(surface, (s) async {
    await s.tester.pumpWidget(s.app(const _Screen(), title: 'Groceries'));
    await s.settle(2);
    open(_page.currentContext!);
    await s.settle();
    await s.take(surface);
    if (timers) {
      // The toasts go by themselves; let them, so no timer outlives the test.
      await s.tester.pump(const Duration(seconds: 5));
      await s.tester.pump(const Duration(seconds: 1));
    }
  });
}

void main() {
  shots('kit-controls', (s) async {
    await s.tester.pumpWidget(s.app(const _Controls(), title: 'Controls'));
    await s.settle();
    await s.take('kit-controls');
  });

  shots('kit-mark', (s) async {
    final started = <ClaxMarkState>{};
    Widget page() => s.app(_Marks({...started}), title: 'Mark');
    await s.tester.pumpWidget(page());
    await s.settle(2);
    // Start each state so that, at the render, it is at its widest.
    final starts = {
      for (final MapEntry(:key, :value) in _peak.entries) key: _render - value,
    };
    for (var t = 0; t <= _render; t += 10) {
      final now = [
        for (final MapEntry(:key, :value) in starts.entries)
          if (value == t) key,
      ];
      if (now.isNotEmpty) {
        started.addAll(now);
        await s.tester.pumpWidget(page());
      }
      if (t < _render) await s.tester.pump(const Duration(milliseconds: 10));
    }
    await s.take('kit-mark');
    // Let the rally and the breaths run out of the test's way.
    await s.tester.pumpWidget(s.app(const SizedBox(), title: 'Mark'));
  });

  _surface(
    'kit-surfaces-sheet',
    (c) => ClaxSheet.show<void>(
      c,
      title: 'Share Groceries',
      builder: (c) => Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(
            'Share sends the app without your list. Share a copy sends '
            'it with everything on it.',
            style: Theme.of(c).textTheme.bodyMedium!.copyWith(
              color: claxPalette(c).muted,
            ),
          ),
          const SizedBox(height: 16),
          const ClaxTextField(label: 'Message', hint: 'Our weekly list'),
        ],
      ),
      actions: [
        ClaxButton(label: 'Cancel', onPressed: () {}),
        ClaxButton(
          label: 'Share',
          onPressed: () {},
          kind: ClaxButtonKind.primary,
          isDefault: true,
        ),
      ],
    ),
  );

  _surface(
    'kit-surfaces-dialog',
    (c) => ClaxDialog.show(
      c,
      title: 'Delete Groceries?',
      body: 'Its list and its history go too. This can’t be undone.',
      actions: const [
        ClaxDialogAction('Cancel'),
        ClaxDialogAction('Delete', kind: ClaxButtonKind.destructive),
      ],
      cancelIndex: 0,
    ),
  );

  _surface(
    'kit-surfaces-menu',
    (c) {
      final box = _more.currentContext!.findRenderObject()! as RenderBox;
      ClaxMenu.show(
        c,
        anchor: box.localToGlobal(Offset.zero) & box.size,
        items: const [
          ClaxMenuItem('rename', 'Rename', icon: Icons.edit_outlined, key: '⌘R'),
          ClaxMenuItem('share', 'Share…', icon: Icons.ios_share),
          ClaxMenuItem('pin', 'Pin to Home', icon: Icons.push_pin_outlined, enabled: false),
          ClaxMenuItem.separator(),
          ClaxMenuItem('delete', 'Delete', icon: Icons.delete_outline, destructive: true),
        ],
      );
    },
  );

  for (final place in ClaxToastPlace.values) {
    _surface(
      'kit-surfaces-toast-${place.name}',
      (c) => ClaxToast.show(
        c,
        'Deleted Oat milk',
        actionLabel: 'Undo',
        onAction: () {},
        place: place,
      ),
      timers: true,
    );
  }
}
