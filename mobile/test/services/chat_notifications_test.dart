import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/protocol/messages.dart';
import 'package:mobile/services/chat_notifications.dart';

void main() {
  group('shouldNotifyFor', () {
    test('resumed (the app is on the chat screen already) never notifies', () {
      expect(shouldNotifyFor(AppLifecycleState.resumed), isFalse);
    });

    test('every other lifecycle state notifies', () {
      for (final state in AppLifecycleState.values) {
        if (state == AppLifecycleState.resumed) continue;
        expect(shouldNotifyFor(state), isTrue, reason: 'expected $state to notify');
      }
    });
  });

  group('notificationContentFor', () {
    test('a chat response uses the server name as the title and the reply as the body', () {
      final result = notificationContentFor(
        const ChatResponseMessage('Your dentist appointment is Friday at 3pm.', null),
        serverName: 'warden-server',
      );
      expect(result.title, 'warden-server');
      expect(result.body, 'Your dentist appointment is Friday at 3pm.');
    });

    test('a chat error is labeled distinctly from a normal reply', () {
      final result = notificationContentFor(
        const ChatErrorMessage('model provider not found'),
        serverName: 'warden-server',
      );
      expect(result.title, 'warden-server — Error');
      expect(result.body, 'model provider not found');
    });

    test('a long body is truncated with an ellipsis', () {
      final longContent = 'a' * 300;
      final result = notificationContentFor(ChatResponseMessage(longContent, null), serverName: 'warden-server');
      expect(result.body.length, 201); // 200 chars + the ellipsis character
      expect(result.body.endsWith('…'), isTrue);
    });
  });
}
