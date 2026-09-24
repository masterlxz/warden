import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/services/hub_pairing_qr.dart';

void main() {
  group('parseHubPairingQr', () {
    test('valid JSON with a well-formed serverUrl parses host/port/authKey', () {
      final result = parseHubPairingQr('{"serverUrl":"ws://192.168.1.10:7420","authKey":"secret"}');
      expect(result, isNotNull);
      expect(result!.host, '192.168.1.10');
      expect(result.port, 7420);
      expect(result.authKey, 'secret');
      expect(result.useTls, isFalse);
    });

    test('a wss:// serverUrl (TLS-only hub, P36) turns TLS on', () {
      final result = parseHubPairingQr('{"serverUrl":"wss://hub.tail1234.ts.net:7420","authKey":"secret"}');
      expect(result, isNotNull);
      expect(result!.host, 'hub.tail1234.ts.net');
      expect(result.port, 7420);
      expect(result.useTls, isTrue);
    });

    test('a serverUrl with a non-WebSocket scheme returns null', () {
      expect(parseHubPairingQr('{"serverUrl":"https://hub.tail1234.ts.net:7420","authKey":"secret"}'), isNull);
    });

    test('garbage that is not JSON returns null', () {
      expect(parseHubPairingQr('not json at all'), isNull);
    });

    test('JSON missing authKey returns null', () {
      expect(parseHubPairingQr('{"serverUrl":"ws://192.168.1.10:7420"}'), isNull);
    });

    test('JSON with an empty authKey returns null', () {
      expect(parseHubPairingQr('{"serverUrl":"ws://192.168.1.10:7420","authKey":""}'), isNull);
    });

    test('serverUrl without a port returns null', () {
      expect(parseHubPairingQr('{"serverUrl":"ws://192.168.1.10","authKey":"secret"}'), isNull);
    });

    test('a JSON array (not an object) returns null', () {
      expect(parseHubPairingQr('["ws://192.168.1.10:7420","secret"]'), isNull);
    });
  });
}
