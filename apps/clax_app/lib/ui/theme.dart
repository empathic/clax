import 'package:flutter/material.dart';

/// The seed of the app's Material 3 color scheme: a quiet slate blue.
const claxSeed = Color(0xFF4A5D8F);

/// The app's theme for [brightness]: Material 3 from [claxSeed], the
/// platform's own page transitions, and roomy controls on touch screens.
ThemeData claxTheme(Brightness brightness) {
  final scheme = ColorScheme.fromSeed(
    seedColor: claxSeed,
    brightness: brightness,
  );
  return ThemeData(
    useMaterial3: true,
    colorScheme: scheme,
    scaffoldBackgroundColor: scheme.surface,
    appBarTheme: AppBarTheme(
      backgroundColor: scheme.surface,
      surfaceTintColor: Colors.transparent,
      scrolledUnderElevation: 0,
    ),
    bottomSheetTheme: BottomSheetThemeData(
      backgroundColor: scheme.surfaceContainerLow,
      showDragHandle: true,
    ),
    inputDecorationTheme: InputDecorationTheme(
      filled: true,
      fillColor: scheme.surfaceContainerHighest,
      border: OutlineInputBorder(
        borderRadius: BorderRadius.circular(16),
        borderSide: BorderSide.none,
      ),
    ),
  );
}
