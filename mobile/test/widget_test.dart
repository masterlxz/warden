import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/main.dart';
import 'package:shared_preferences/shared_preferences.dart';

void main() {
  testWidgets('ConnectionScreen renders the connection form', (tester) async {
    // ConnectionScreen loads persisted settings via shared_preferences on
    // init; without a mocked backend the plugin channel has no native side
    // to answer in a widget test.
    SharedPreferences.setMockInitialValues({});

    await tester.pumpWidget(const WardenApp());
    await tester.pump();

    expect(find.text('Warden — Server Connection'), findsOneWidget);
    expect(find.widgetWithText(TextField, 'Server host'), findsOneWidget);
    expect(find.widgetWithText(TextField, 'Port'), findsOneWidget);
    expect(find.widgetWithText(TextField, 'Auth key'), findsOneWidget);
    expect(find.widgetWithText(TextField, 'Device name'), findsOneWidget);
    expect(find.widgetWithText(FilledButton, 'Connect'), findsOneWidget);
    expect(find.text('Disconnected'), findsOneWidget);
  });
}
