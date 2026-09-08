import 'package:flutter/material.dart';

import 'screens/connection_screen.dart';
import 'services/chat_notifications.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  // Fase 7.5 — flutter_local_notifications talks over a MethodChannel, which asserts the binary
  // messenger is ready before any call — required here (and not before) because this is the first
  // plugin in main() that goes through a MethodChannel; RustLib.init() below is plain FFI, not a
  // platform channel, so it never needed this.
  WidgetsFlutterBinding.ensureInitialized();
  // Fase 4.4 — loads the native warden_mobile_bridge library (Rust: sync engine + vault) before
  // any screen can call into it. Must finish before runApp, same reasoning as any other native
  // plugin registration.
  await RustLib.init();
  // Fase 7.5 — registers the Android notification channel before any screen could try to notify.
  await initializeChatNotifications();
  runApp(const WardenApp());
}

class WardenApp extends StatelessWidget {
  const WardenApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Warden',
      // Matches desktop/src/App.css --color-accent (light #7c3aed, dark #a78bfa) instead of the
      // generic Material seed, so the two apps read as the same brand.
      theme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xFF7C3AED)),
      ),
      darkTheme: ThemeData(
        colorScheme: ColorScheme.fromSeed(seedColor: const Color(0xFFA78BFA), brightness: Brightness.dark),
      ),
      themeMode: ThemeMode.system,
      home: const ConnectionScreen(),
    );
  }
}
