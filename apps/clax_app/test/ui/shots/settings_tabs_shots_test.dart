// The Mac's Settings window rendered tab by tab into CLAX_SHOTS_DIR (skipped
// without it), light and dark: `settings-window-<tab>-mac-<theme>.png`, each
// at its content's height under a stand-in for the window's preference
// toolbar (its tabs with their labels, the shown one selected), which
// flutter_test cannot draw. Claude also with a saved key and the Claude Code
// agent, Advanced also with Developer tools on.
import 'dart:io';
import 'dart:math';

import 'package:clax_app/agent/timings.dart';
import 'package:clax_app/home/home_settings.dart';
import 'package:clax_app/platform/secure_key.dart';
import 'package:clax_app/platform/settings_store.dart';
import 'package:clax_app/ui/developer.dart';
import 'package:clax_app/ui/haiku.dart';
import 'package:clax_app/ui/settings/settings_tabs.dart';
import 'package:clax_app/ui/settings_screen.dart';
import 'package:clax_app/ui/theme.dart';
import 'package:clax_app/ui/tokens.g.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

import 'rig.dart';

class _Key implements SecureKey {
  _Key(this.value);
  String? value;

  @override
  Future<String?> read() async => value;

  @override
  Future<void> write(String? key) async => value = key;
}

/// The Settings window: 600 wide, as tall as its tallest tab here.
const _window = ShotForm('mac', Size(600, 600), 2, TargetPlatform.macOS);

/// The window's preference toolbar, as AppKit draws it: the window buttons,
/// the title centred over the tabs, each a symbol over its label, the
/// shown one in a rounded selection.
class _PreferenceBand extends StatelessWidget {
  const _PreferenceBand({required this.tab});

  final SettingsTab tab;

  static const _icons = {
    SettingsTab.general: Icons.settings_outlined,
    SettingsTab.claude: Icons.key_outlined,
    SettingsTab.dictation: Icons.mic_none_rounded,
    SettingsTab.sharing: Icons.people_outline_rounded,
    SettingsTab.notifications: Icons.notifications_none_rounded,
    SettingsTab.homeScreen: Icons.home_outlined,
    SettingsTab.advanced: Icons.handyman_outlined,
  };

  @override
  Widget build(BuildContext context) {
    final dark = Theme.of(context).brightness == Brightness.dark;
    final p = dark ? ClaxTokens.dark : ClaxTokens.light;
    final small = ClaxType.of(ClaxRole.caption, TargetPlatform.macOS);
    Widget light(Color c) => Container(
      width: 12,
      height: 12,
      margin: const EdgeInsets.only(right: 8),
      decoration: BoxDecoration(color: c, shape: BoxShape.circle),
    );
    return Material(
      color: p.card,
      child: Container(
        height: 86,
        decoration: BoxDecoration(
          border: Border(bottom: BorderSide(color: p.border)),
        ),
        child: Stack(
          children: [
            Positioned(
              left: 12,
              top: 10,
              child: Row(
                children: [
                  light(const Color(0xFFFF5F57)),
                  light(p.surfaceContainers[3]),
                  light(p.surfaceContainers[3]),
                ],
              ),
            ),
            Positioned(
              left: 0,
              right: 0,
              top: 7,
              child: Text(
                tab.title,
                textAlign: TextAlign.center,
                style: ClaxType.of(
                  ClaxRole.headline,
                  TargetPlatform.macOS,
                ).copyWith(color: p.fg),
              ),
            ),
            Positioned(
              left: 0,
              right: 0,
              top: 32,
              child: Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  for (final t in SettingsTab.values)
                    Container(
                      width: 74,
                      height: 48,
                      margin: const EdgeInsets.symmetric(horizontal: 1),
                      decoration: BoxDecoration(
                        color: t == tab ? p.surfaceContainers[2] : null,
                        borderRadius: BorderRadius.circular(6),
                      ),
                      child: Column(
                        mainAxisAlignment: MainAxisAlignment.center,
                        children: [
                          Icon(
                            _icons[t],
                            size: 20,
                            color: t == tab ? p.agent : p.muted,
                          ),
                          const SizedBox(height: 2),
                          Text(
                            t.title,
                            style: small.copyWith(
                              color: t == tab ? p.agent : p.muted,
                            ),
                          ),
                        ],
                      ),
                    ),
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }
}

Future<void> _tab(
  Shot s,
  SettingsTab tab, {
  String? surface,
  String? key,
  bool developer = false,
  bool cli = false,
}) async {
  final dir = Directory.systemTemp.createTempSync('clax-shots-settings-tab-');
  addTearDown(() => dir.deleteSync(recursive: true));
  final store = SettingsStore(File('${dir.path}/settings.json'));
  await s.tester.runAsync(
    () => store.save(
      AppSettings(authorName: 'Alex Kesling')
        ..developer = developer
        ..backend = cli ? backendClaudeCli : backendMessagesApi,
    ),
  );
  final dev = ValueNotifier(developer);
  addTearDown(dev.dispose);
  final haikus = await s.tester.runAsync(() => loadHaikus(rootBundle));
  final now = DateTime(2026, 10, 19, 9, 41);
  await s.tester.pumpWidget(
    DeveloperScope(
      notifier: dev,
      child: MaterialApp(
        debugShowCheckedModeBanner: false,
        theme: claxTheme(Brightness.light, TargetPlatform.macOS),
        darkTheme: claxTheme(Brightness.dark, TargetPlatform.macOS),
        themeMode: s.dark ? ThemeMode.dark : ThemeMode.light,
        home: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _PreferenceBand(tab: tab),
            Expanded(
              child: SettingsTabs(
                tab: tab,
                screen: (t) => SettingsScreen(
                  tab: t,
                  store: store,
                  secureKey: _Key(key),
                  cliSupported: true,
                  timings: [
                    TimingRecord(
                      name: sayUsable,
                      at: now,
                      elapsed: const Duration(milliseconds: 8400),
                    ),
                    TimingRecord(
                      name: homeUsable,
                      at: now.subtract(const Duration(minutes: 3)),
                      elapsed: const Duration(milliseconds: 1200),
                    ),
                  ],
                  fingerprint: () async => '7KQ2-M9XD-4FTA-QW3B',
                  home: HomeScreenControls(
                    settings: HomeSettings.loadSync(dir)..homeAppId = 'home',
                    open: () {},
                    changed: () {},
                  ),
                  homeVersion: () async => '1.6',
                  haikus: () async => haikus!,
                  random: Random(4),
                  notificationsAllowed: () async => false,
                ),
              ),
            ),
          ],
        ),
      ),
    ),
  );
  await s.tester.runAsync(() => Future<void>.delayed(Duration.zero));
  await s.settle();
  await s.take(surface ?? 'settings-window-${tab.name}');
}

void main() {
  for (final tab in SettingsTab.values) {
    shots(
      'settings-window-${tab.name}',
      (s) => _tab(s, tab),
      forms: const [_window],
    );
  }
  shots(
    'settings-window-claude-saved',
    (s) => _tab(
      s,
      SettingsTab.claude,
      surface: 'settings-window-claude-saved',
      key: 'sk-ant-api03-shots',
      cli: true,
    ),
    forms: const [_window],
  );
  shots(
    'settings-window-advanced-on',
    (s) => _tab(
      s,
      SettingsTab.advanced,
      surface: 'settings-window-advanced-on',
      developer: true,
    ),
    forms: const [_window],
  );
}
