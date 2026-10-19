// The kit's components rendered in every form and theme into
// CLAX_SHOTS_DIR (skipped without it): `kit-controls` is one page with
// every control in each of its states.
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

void main() {
  shots('kit-controls', (s) async {
    await s.tester.pumpWidget(s.app(const _Controls(), title: 'Controls'));
    await s.settle();
    await s.take('kit-controls');
  });
}
