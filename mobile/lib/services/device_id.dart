import 'dart:math';

/// Generates a stable-ish random device identifier. Not a cryptographic
/// identity, just needs to be unique-enough per install — the server treats
/// `deviceId` as an opaque string, so a full UUID-formatted value buys
/// nothing over 16 random bytes hex-encoded (same 128 bits of entropy as a
/// v4 UUID's random bits, without pulling in the `uuid` package).
String generateDeviceId() {
  final random = Random.secure();
  final bytes = List<int>.generate(16, (_) => random.nextInt(256));
  return bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join();
}
