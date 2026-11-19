import 'package:factory_app/settings_view.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('token counts parse the way they are shown', () {
    expect(parseTokens('400k'), 400000);
    expect(parseTokens(' 1.2M '), 1200000);
    expect(parseTokens('250 000'), 250000);
    expect(parseTokens('180,000'), 180000);
    expect(parseTokens('lots'), isNull);
    expect(parseTokens(''), isNull);
  });
}
