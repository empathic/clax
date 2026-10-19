import 'package:flutter/cupertino.dart' show CupertinoPageTransitionsBuilder;
import 'package:flutter/material.dart';

import 'kit/family.dart';
import 'tokens.g.dart';

/// The brand's palette carried on a [ThemeData], so a widget reads the
/// tokens the theme was built from ([claxPalette]).
final class ClaxColors extends ThemeExtension<ClaxColors> {
  const ClaxColors(this.palette);
  final ClaxPalette palette;

  @override
  ClaxColors copyWith({ClaxPalette? palette}) =>
      ClaxColors(palette ?? this.palette);

  /// Palettes do not blend: the theme switches at the half-way point.
  @override
  ClaxColors lerp(ClaxColors? other, double t) =>
      other == null || t < 0.5 ? this : other;

  /// Equal when they carry the same palette: the palettes are the constants
  /// of [ClaxTokens], so a theme built again equals the one before it and an
  /// `AnimatedTheme` has nothing to animate.
  @override
  bool operator ==(Object other) =>
      other is ClaxColors && identical(other.palette, palette);

  @override
  int get hashCode => identityHashCode(palette);
}

/// The palette of the theme at [context]; the light palette under a theme
/// [claxTheme] did not build.
ClaxPalette claxPalette(BuildContext context) {
  final theme = Theme.of(context);
  return theme.extension<ClaxColors>()?.palette ??
      (theme.brightness == Brightness.dark
          ? ClaxTokens.dark
          : ClaxTokens.light);
}

/// The Material type scale on [platform] from the brand's roles, in
/// [p]'s `fg`, letter-spacing 0: `displaySmall` (and the larger display
/// and headline sizes) is `large-title`; `headlineSmall` and `titleLarge`
/// `title-1`; `titleMedium` `title-2`; `titleSmall` `title-3`; `bodyLarge`
/// `body-l`; `bodyMedium` `body`; `bodySmall` and `labelSmall` `caption`;
/// `labelLarge` (buttons) `body` at weight 500; `labelMedium` `callout` at
/// weight 500.
TextTheme claxTextTheme(TargetPlatform platform, ClaxPalette p) {
  TextStyle r(ClaxRole role, [FontWeight? weight]) => ClaxType.of(
    role,
    platform,
  ).copyWith(color: p.fg, fontWeight: weight);
  return TextTheme(
    displayLarge: r(ClaxRole.largeTitle),
    displayMedium: r(ClaxRole.largeTitle),
    displaySmall: r(ClaxRole.largeTitle),
    headlineLarge: r(ClaxRole.largeTitle),
    headlineMedium: r(ClaxRole.largeTitle),
    headlineSmall: r(ClaxRole.title1),
    titleLarge: r(ClaxRole.title1),
    titleMedium: r(ClaxRole.title2),
    titleSmall: r(ClaxRole.title3),
    bodyLarge: r(ClaxRole.bodyL),
    bodyMedium: r(ClaxRole.body),
    bodySmall: r(ClaxRole.caption),
    labelLarge: r(ClaxRole.body, FontWeight.w500),
    labelMedium: r(ClaxRole.callout, FontWeight.w500),
    labelSmall: r(ClaxRole.caption),
  );
}

/// The color scheme from [p], by hand (the spec's §1.2 table): green
/// primary for Clax and the controls, ink secondary, red-orange tertiary
/// for people, the warm neutrals as surfaces, and no surface tint.
ColorScheme _scheme(Brightness b, ClaxPalette p) => ColorScheme(
  brightness: b,
  primary: p.agent,
  onPrimary: p.onAccent,
  primaryContainer: p.accentTint,
  onPrimaryContainer: p.agentInk,
  secondary: p.fg,
  onSecondary: p.bg,
  // Material's tonal controls (FilledButton.tonal, the navigation
  // indicator) fill with the secondary container: Clax's tint.
  secondaryContainer: p.accentTint,
  onSecondaryContainer: p.agentInk,
  tertiary: p.you,
  onTertiary: p.onYou,
  tertiaryContainer: p.commentHl,
  onTertiaryContainer: p.youInk,
  error: p.danger,
  onError: p.onDanger,
  errorContainer: p.dangerTint,
  onErrorContainer: p.danger,
  surface: p.bg,
  onSurface: p.fg,
  onSurfaceVariant: p.muted,
  surfaceContainerLowest: p.surfaceContainers[0],
  surfaceContainerLow: p.surfaceContainers[1],
  surfaceContainer: p.surfaceContainers[2],
  surfaceContainerHigh: p.surfaceContainers[3],
  surfaceContainerHighest: p.surfaceContainers[4],
  outline: p.borderStrong,
  outlineVariant: p.border,
  inverseSurface: p.fg,
  onInverseSurface: p.bg,
  inversePrimary: p.inversePrimary,
  surfaceTint: Colors.transparent,
  shadow: Colors.black,
  scrim: p.scrim,
);

/// The app's theme in [brightness] on [platform], every colour, size and
/// radius from the brand's tokens; [highContrast] takes the palettes with
/// more contrast. The Material
/// family is standard density with predictive back on Android; the Apple
/// family (macOS, iOS) is compact with Apple's type sizes.
///
/// Each combination is built once and the same object returned after: its
/// state-dependent styles are closures, which no two builds share, so two
/// builds would never be equal and an `AnimatedTheme` given a new build on
/// every rebuild would animate (and repaint) each time.
ThemeData claxTheme(
  Brightness brightness,
  TargetPlatform platform, {
  bool highContrast = false,
}) => _themes.putIfAbsent(
  (brightness, platform, highContrast),
  () => _buildTheme(brightness, platform, highContrast: highContrast),
);

final _themes = <(Brightness, TargetPlatform, bool), ThemeData>{};

ThemeData _buildTheme(
  Brightness brightness,
  TargetPlatform platform, {
  required bool highContrast,
}) {
  final dark = brightness == Brightness.dark;
  final p = switch ((dark, highContrast)) {
    (false, false) => ClaxTokens.light,
    (true, false) => ClaxTokens.dark,
    (false, true) => ClaxTokens.lightMore,
    (true, true) => ClaxTokens.darkMore,
  };
  final apple = ClaxFamily.forPlatform(platform) == ClaxFamily.apple;
  final scheme = _scheme(brightness, p);
  // The roles over the platform's typography, so every style a theme below
  // hands out carries the platform's family as well as the brand's sizes.
  final typography = Typography.material2021(platform: platform);
  final text = (dark ? typography.white : typography.black).merge(
    claxTextTheme(platform, p),
  );
  final controlRadius = BorderRadius.circular(
    apple ? ClaxRadius.controlApple : ClaxRadius.sm,
  );
  OutlineInputBorder outline(Color c, [double width = 1]) => OutlineInputBorder(
    borderRadius: controlRadius,
    borderSide: BorderSide(color: c, width: width),
  );
  // Green when on: the switch's track, the checkbox's and radio's fill.
  WidgetStateProperty<Color?> onGreen(Color? off) =>
      WidgetStateProperty.resolveWith(
        (s) => s.contains(WidgetState.disabled)
            ? null
            : s.contains(WidgetState.selected)
            ? p.agent
            : off,
      );
  return ThemeData(
    useMaterial3: true,
    brightness: brightness,
    platform: platform,
    colorScheme: scheme,
    textTheme: text,
    visualDensity: apple ? VisualDensity.compact : VisualDensity.standard,
    scaffoldBackgroundColor: p.bg,
    canvasColor: p.bg,
    dividerColor: p.border,
    hoverColor: p.hover,
    extensions: [ClaxColors(p)],
    textSelectionTheme: TextSelectionThemeData(
      cursorColor: p.agent,
      // Flutter cannot recolour selected text, so the selection is a tint
      // the text keeps reading on (opaque pink in light, translucent in dark).
      selectionColor: p.selectionTint,
      selectionHandleColor: p.agent,
    ),
    pageTransitionsTheme: const PageTransitionsTheme(
      builders: {
        TargetPlatform.android: PredictiveBackPageTransitionsBuilder(),
        TargetPlatform.iOS: CupertinoPageTransitionsBuilder(),
        TargetPlatform.macOS: CupertinoPageTransitionsBuilder(),
      },
    ),
    appBarTheme: AppBarThemeData(
      backgroundColor: p.bg,
      foregroundColor: p.fg,
      surfaceTintColor: Colors.transparent,
      scrolledUnderElevation: 0,
      titleTextStyle: text.titleLarge,
    ),
    cardTheme: CardThemeData(
      color: p.card,
      surfaceTintColor: Colors.transparent,
      elevation: dark ? 0 : 1,
      shadowColor: const Color(0x2914140F),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(ClaxRadius.md),
        side: dark ? BorderSide(color: p.border) : BorderSide.none,
      ),
    ),
    dividerTheme: DividerThemeData(color: p.border, space: 1, thickness: 1),
    filledButtonTheme: FilledButtonThemeData(
      style: ButtonStyle(
        backgroundColor: WidgetStateProperty.resolveWith(
          (s) => s.contains(WidgetState.disabled)
              ? p.fg.withValues(alpha: 0.12)
              : s.contains(WidgetState.hovered) ||
                    s.contains(WidgetState.pressed)
              ? p.primaryHover
              : p.fg,
        ),
        foregroundColor: WidgetStateProperty.resolveWith(
          (s) => s.contains(WidgetState.disabled)
              ? p.fg.withValues(alpha: 0.38)
              : p.bg,
        ),
        overlayColor: const WidgetStatePropertyAll(Colors.transparent),
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(borderRadius: controlRadius),
        ),
        textStyle: WidgetStatePropertyAll(text.labelLarge),
      ),
    ),
    textButtonTheme: TextButtonThemeData(
      style: ButtonStyle(
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(borderRadius: controlRadius),
        ),
        textStyle: WidgetStatePropertyAll(text.labelLarge),
      ),
    ),
    outlinedButtonTheme: OutlinedButtonThemeData(
      style: ButtonStyle(
        foregroundColor: WidgetStatePropertyAll(p.fg),
        side: WidgetStatePropertyAll(BorderSide(color: p.borderStrong)),
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(borderRadius: controlRadius),
        ),
        textStyle: WidgetStatePropertyAll(text.labelLarge),
      ),
    ),
    inputDecorationTheme: InputDecorationThemeData(
      filled: true,
      fillColor: p.card,
      hoverColor: Colors.transparent,
      floatingLabelBehavior: FloatingLabelBehavior.auto,
      labelStyle: text.bodyLarge!.copyWith(color: p.muted),
      floatingLabelStyle: WidgetStateTextStyle.resolveWith(
        (s) => text.bodySmall!.copyWith(
          color: s.contains(WidgetState.error)
              ? p.danger
              : s.contains(WidgetState.focused)
              ? p.agentInk
              : p.muted,
        ),
      ),
      hintStyle: text.bodyLarge!.copyWith(color: p.muted),
      helperStyle: text.bodySmall!.copyWith(color: p.muted),
      errorStyle: text.bodySmall!.copyWith(color: p.danger),
      border: outline(p.borderStrong),
      enabledBorder: outline(p.borderStrong),
      disabledBorder: outline(p.border),
      focusedBorder: outline(p.agent, 2),
      errorBorder: outline(p.danger),
      focusedErrorBorder: outline(p.danger, 2),
    ),
    snackBarTheme: SnackBarThemeData(
      behavior: SnackBarBehavior.floating,
      // No width: a floating bar with one drops its side margins, so on a
      // phone it would run edge to edge. ClaxToast caps its own at 560.
      insetPadding: const EdgeInsets.fromLTRB(
        ClaxSpace.s16,
        ClaxSpace.s4,
        ClaxSpace.s16,
        ClaxSpace.s12,
      ),
      backgroundColor: p.fg,
      contentTextStyle: text.bodyMedium!.copyWith(color: p.bg),
      actionTextColor: p.inversePrimary,
      elevation: 2,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(ClaxRadius.sm),
      ),
    ),
    dialogTheme: DialogThemeData(
      backgroundColor: p.raised,
      surfaceTintColor: Colors.transparent,
      elevation: dark ? 0 : 3,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(
          apple ? ClaxRadius.md : ClaxRadius.xl,
        ),
        side: dark ? BorderSide(color: p.borderHover) : BorderSide.none,
      ),
      titleTextStyle: text.titleMedium,
      contentTextStyle: text.bodyMedium!.copyWith(color: p.muted),
    ),
    bottomSheetTheme: BottomSheetThemeData(
      backgroundColor: p.raised,
      modalBackgroundColor: p.raised,
      surfaceTintColor: Colors.transparent,
      showDragHandle: true,
      dragHandleColor: p.borderStrong,
      modalBarrierColor: p.scrim,
      shape: RoundedRectangleBorder(
        borderRadius: const BorderRadius.vertical(
          top: Radius.circular(ClaxRadius.xl),
        ),
        side: dark ? BorderSide(color: p.borderHover) : BorderSide.none,
      ),
    ),
    listTileTheme: ListTileThemeData(
      contentPadding: const EdgeInsets.symmetric(horizontal: ClaxSpace.s16),
      iconColor: p.muted,
      titleTextStyle: text.bodyLarge,
      subtitleTextStyle: text.bodyMedium!.copyWith(color: p.muted),
    ),
    chipTheme: ChipThemeData(
      backgroundColor: p.card,
      selectedColor: p.accentTint,
      checkmarkColor: p.agentInk,
      side: BorderSide(color: p.border),
      labelStyle: text.bodyMedium,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(ClaxRadius.sm),
      ),
    ),
    menuTheme: MenuThemeData(
      style: MenuStyle(
        backgroundColor: WidgetStatePropertyAll(p.overlay),
        surfaceTintColor: const WidgetStatePropertyAll(Colors.transparent),
        elevation: WidgetStatePropertyAll(dark ? 0 : 2),
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(ClaxRadius.md),
            side: dark ? BorderSide(color: p.borderHover) : BorderSide.none,
          ),
        ),
      ),
    ),
    popupMenuTheme: PopupMenuThemeData(
      color: p.overlay,
      surfaceTintColor: Colors.transparent,
      elevation: dark ? 0 : 2,
      textStyle: text.bodyLarge,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(ClaxRadius.md),
        side: dark ? BorderSide(color: p.borderHover) : BorderSide.none,
      ),
    ),
    tooltipTheme: TooltipThemeData(
      decoration: BoxDecoration(
        color: p.fg,
        borderRadius: BorderRadius.circular(ClaxRadius.xs),
      ),
      textStyle: text.bodySmall!.copyWith(color: p.bg),
    ),
    badgeTheme: BadgeThemeData(
      backgroundColor: p.you,
      textColor: p.onYou,
      textStyle: ClaxType.of(ClaxRole.caption, platform),
    ),
    progressIndicatorTheme: ProgressIndicatorThemeData(
      color: p.agent,
      linearTrackColor: p.accentTint,
      circularTrackColor: Colors.transparent,
    ),
    switchTheme: SwitchThemeData(
      trackColor: onGreen(null),
      thumbColor: WidgetStateProperty.resolveWith(
        (s) => s.contains(WidgetState.disabled)
            ? null
            : s.contains(WidgetState.selected)
            ? p.onAccent
            : p.borderStrong,
      ),
      trackOutlineColor: WidgetStateProperty.resolveWith(
        (s) => s.contains(WidgetState.selected) ? Colors.transparent : null,
      ),
    ),
    checkboxTheme: CheckboxThemeData(
      fillColor: onGreen(Colors.transparent),
      checkColor: WidgetStatePropertyAll(p.onAccent),
      side: WidgetStateBorderSide.resolveWith(
        (s) => s.contains(WidgetState.selected)
            ? BorderSide.none
            : BorderSide(color: p.borderStrong, width: 2),
      ),
    ),
    radioTheme: RadioThemeData(fillColor: onGreen(p.borderStrong)),
    floatingActionButtonTheme: FloatingActionButtonThemeData(
      backgroundColor: p.accentTint,
      foregroundColor: p.agentInk,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(ClaxRadius.lg),
      ),
    ),
    navigationBarTheme: NavigationBarThemeData(
      backgroundColor: p.surfaceContainers[2],
      surfaceTintColor: Colors.transparent,
      indicatorColor: p.accentTint,
    ),
  );
}
