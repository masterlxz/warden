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
      theme: ThemeData(colorScheme: ColorScheme.fromSeed(seedColor: Colors.deepPurple)),
      home: const ConnectionScreen(),
    );
  }
}
