import 'package:flutter/material.dart';

void main() {
  runApp(const ClaxApp());
}

/// The app's root widget until the full composition replaces it: a plain
/// screen showing the app's name.
class ClaxApp extends StatelessWidget {
  const ClaxApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Clax',
      theme: ThemeData(useMaterial3: true),
      darkTheme: ThemeData(useMaterial3: true, brightness: Brightness.dark),
      home: const Scaffold(body: Center(child: Text('Clax'))),
    );
  }
}
