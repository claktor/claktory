import 'package:factory_app/backend.dart';
import 'package:factory_app/login.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:flutter_test/flutter_test.dart';

Workspace flowing({
  String provider = 'anthropic',
  String url = '',
  String error = '',
}) => Workspace(
  login: LoginFlow(
    providerId: provider,
    methodId: 'oauth',
    url: url,
    error: error,
  ),
);

void main() {
  late WorkspaceStore store;
  late List<String> opened;
  late LoginLauncher launcher;

  setUp(() {
    store = WorkspaceStore();
    opened = [];
    launcher = LoginLauncher(
      workspace: store,
      send: (_) async {},
      open: (url) async {
        opened.add('$url');
        return true;
      },
    );
  });

  test('opens a flow started here once, when its address arrives', () async {
    await launcher.start('anthropic', 'oauth');
    store.apply(flowing());
    store.apply(flowing(url: 'https://a.test/1'));
    store.apply(flowing(url: 'https://a.test/1'));

    expect(opened, ['https://a.test/1']);
    expect(launcher.opened('https://a.test/1'), isTrue);
  });

  test("doesn't open flows another window started", () {
    store.apply(flowing(url: 'https://a.test/1'));
    expect(opened, isEmpty);
    expect(launcher.opened('https://a.test/1'), isFalse);
  });

  test("doesn't open the address of the flow being replaced", () async {
    store.apply(flowing(url: 'https://a.test/old'));
    await launcher.start('anthropic', 'oauth');
    store.apply(flowing(url: 'https://a.test/old'));
    store.apply(flowing(url: 'https://a.test/new'));

    expect(opened, ['https://a.test/new']);
  });

  test('a failed, cancelled or refused start opens nothing later', () async {
    await launcher.start('anthropic', 'oauth');
    store.apply(flowing(error: 'denied'));
    store.apply(flowing(url: 'https://a.test/1'));

    await launcher.start('anthropic', 'oauth');
    launcher.forget();
    store.apply(flowing(url: 'https://a.test/2'));

    final refusing = LoginLauncher(
      workspace: store,
      send: (_) => Future.error(const RequestFailure('no')),
      open: (url) async {
        opened.add('$url');
        return true;
      },
    );
    await expectLater(
      refusing.start('anthropic', 'oauth'),
      throwsA(isA<RequestFailure>()),
    );
    store.apply(flowing(url: 'https://a.test/3'));

    expect(opened, isEmpty);
  });
}
