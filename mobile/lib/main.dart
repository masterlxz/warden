import 'package:flutter/material.dart';

import 'screens/connection_screen.dart';

void main() {
  runApp(const WardenApp());
}

class WardenApp extends StatelessWidget {
  const WardenApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Warden',
      theme: ThemeData(colorScheme: ColorScheme.fromSeed(seedColor: Colors.deepPurple)),
      home: const ConnectionScreen(),
    );
  }
}
