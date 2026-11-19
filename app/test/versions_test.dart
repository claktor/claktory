import 'package:factory_app/update.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('numbered versions compare part by part, not as text', () {
    expect(isNewer('v0.1.10', 'v0.1.9'), isTrue);
    expect(isNewer('v0.1.9', 'v0.1.10'), isFalse);
    expect(isNewer('v0.2.0', 'v0.1.500'), isTrue);
    expect(isNewer('v1.0.0', 'v0.99.99'), isTrue);
    expect(isNewer('v0.1.3', 'v0.1.3'), isFalse);
  });

  test('a date stamp is older than any numbered version', () {
    expect(isNewer('v0.1.0', '2026.11.19-2200'), isTrue);
    expect(isNewer('2026.11.19-2200', 'v0.1.0'), isFalse);
    expect(isNewer('2026.11.19-2200', '2026.11.18-1600'), isTrue);
  });

  test('a build before a release comes just before it', () {
    expect(isNewer('v0.1.3', 'v0.1.3-dev.x'), isTrue);
    expect(isNewer('v0.1.3-dev.x', 'v0.1.3'), isFalse);
    expect(isNewer('v0.1.3-dev.x', 'v0.1.2'), isTrue);
    expect(
      isNewer('v0.1.3-dev.20261120-1012', 'v0.1.3-dev.20261120-0900'),
      isTrue,
    );
  });

  test('dev is newest and never offered an update', () {
    expect(isNewer('v9.9.9', 'dev'), isFalse);
    expect(isNewer('dev', 'v9.9.9'), isTrue);
    expect(isNewer('dev', 'dev'), isFalse);
    expect(isNewer('', 'v0.1.0'), isFalse);
  });
}
