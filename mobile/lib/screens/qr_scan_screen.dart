import 'package:flutter/material.dart';
import 'package:mobile_scanner/mobile_scanner.dart';

import '../services/hub_pairing_qr.dart';

/// Fase 9.7 — scans the QR the desktop Workspace screen shows, so `ConnectionScreen` doesn't need
/// the server host/port/auth key typed by hand. Pops with a [HubPairingPayload] on a valid scan;
/// pops with `null` if the user backs out without one.
class QrScanScreen extends StatefulWidget {
  const QrScanScreen({super.key});

  @override
  State<QrScanScreen> createState() => _QrScanScreenState();
}

class _QrScanScreenState extends State<QrScanScreen> {
  final _controller = MobileScannerController();
  String? _error;
  bool _handled = false;

  void _onDetect(BarcodeCapture capture) {
    if (_handled) return;
    if (capture.barcodes.isEmpty) return;
    final raw = capture.barcodes.first.rawValue;
    if (raw == null) return;

    final payload = parseHubPairingQr(raw);
    if (payload == null) {
      setState(() => _error = 'QR code não reconhecido — escaneie o QR do Workspace do desktop.');
      return;
    }

    _handled = true;
    Navigator.of(context).pop(payload);
  }

  @override
  void dispose() {
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text('Escanear QR do hub')),
      body: Stack(
        children: [
          MobileScanner(controller: _controller, onDetect: _onDetect),
          if (_error != null)
            Positioned(
              left: 16,
              right: 16,
              bottom: 24,
              child: Material(
                color: Colors.black87,
                borderRadius: BorderRadius.circular(8),
                child: Padding(
                  padding: const EdgeInsets.all(12),
                  child: Text(_error!, style: const TextStyle(color: Colors.white), textAlign: TextAlign.center),
                ),
              ),
            ),
        ],
      ),
    );
  }
}
