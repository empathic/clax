import 'package:clax_app/main.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('the app starts and shows its name', (tester) async {
    await tester.pumpWidget(const ClaxApp());
    expect(find.text('Clax'), findsOneWidget);
  });
}
