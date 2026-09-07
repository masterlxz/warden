import 'package:flutter/material.dart';

import 'screens/connection_screen.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  // Fase 4.4 — loads the native warden_mobile_bridge library (Rust: sync engine + vault) before
  // any screen can call into it. Must finish before runApp, same reasoning as any other native
  // plugin registration.
  await RustLib.init();
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
