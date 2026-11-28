import 'dart:async';
import 'dart:io';

import 'package:factory_app/app_views/transport.dart';
import 'package:factory_app/backend.dart';
import 'package:factory_app/protocol/factory/v1/factory.pb.dart';
import 'package:factory_app/state.dart';
import 'package:fixnum/fixnum.dart';
import 'package:flutter_test/flutter_test.dart';

import 'network_test.dart' show until;
import 'tunnel_test.dart' show FakeTunnel, FakeTunnels;

/// The backend's end: welcomes, answers OpenAppView with the surface, then
/// sends a change of state and an update unasked; records the rest.
class _Far {
  late final HttpServer server;
  final asked = <ClientMessage>[];

  Future<void> start() async {
    server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    server.listen((request) async {
      final socket = await WebSocketTransformer.upgrade(request);
      socket.listen((frame) {
        final message = ClientMessage.fromBuffer(frame as List<int>);
        if (message.hasHello()) {
          socket.add(
            ServerMessage(
              replyTo: message.id,
              welcome: Welcome(serverVersion: 'test'),
            ).writeToBuffer(),
          );
          return;
        }
        asked.add(message);
        if (message.hasOpenAppView()) {
          socket
            ..add(
              ServerMessage(
                replyTo: message.id,
                appSurface: AppSurface(
                  surfaceId: 'charts-1',
                  appId: 'charts',
                  title: 'Charts',
                  state: AppSurfaceState.APP_SURFACE_STATE_OPEN,
                  seq: Int64(3),
                  snapshot: '{"version":"1.0","createSurface":{}}',
                ),
              ).writeToBuffer(),
            )
            ..add(
              ServerMessage(
                appViewUpdate: AppViewUpdate(
                  surfaceId: 'charts-1',
                  seq: Int64(4),
                  message: '{"version":"1.0","deleteSurface":{}}',
                ),
              ).writeToBuffer(),
            )
            ..add(
              ServerMessage(
                appSurface: AppSurface(
                  surfaceId: 'charts-1',
                  appId: 'charts',
                  state: AppSurfaceState.APP_SURFACE_STATE_STOPPED,
                  error: 'Charts stopped.',
                ),
              ).writeToBuffer(),
            );
          return;
        }
        socket.add(
          ServerMessage(replyTo: message.id, ack: Ack()).writeToBuffer(),
        );
      });
    });
  }

  Future<void> close() => server.close(force: true);
}

void main() {
  test("the backend's connection carries app views: the open answered with "
      'the surface, then its changes as events; actions and closing go as '
      'requests', () async {
    final far = _Far();
    await far.start();
    final backend = Backend(
      workspace: WorkspaceStore(),
      transcript: TranscriptStore(),
      explorer: ExplorerStore(),
      extensionView: ExtensionViewStore(),
      tunnels: FakeTunnels(FakeTunnel(0)),
      localEndpoint: 'ws://127.0.0.1:${far.server.port}',
    );
    final AppViewTransport transport = backend;
    final events = <AppViewEvent>[];
    transport.viewEvents.listen(events.add);
    unawaited(backend.run());
    await until(() => transport.viewsConnected.value);

    final surface = await transport.openView(
      appId: 'charts',
      view: 'dashboard',
      params: '{"id":"d1"}',
      catalogs: appViewCatalogs,
    );
    final open = far.asked.single.openAppView;
    expect(open.appId, 'charts');
    expect(open.view, 'dashboard');
    expect(open.params, '{"id":"d1"}');
    expect(open.catalogs, [appViewCatalogV1]);
    expect(surface.surfaceId, 'charts-1');
    expect(surface.state, AppViewState.open);
    expect(surface.seq, 3);

    await until(() => events.length == 2);
    final update = (events[0] as AppSurfaceUpdated).update;
    expect(update.seq, 4);
    expect(update.message, contains('deleteSurface'));
    final changed = (events[1] as AppSurfaceChanged).surface;
    expect(changed.state, AppViewState.stopped);
    expect(changed.error, 'Charts stopped.');

    await transport.actInView('charts-1', '{"version":"1.0","action":{}}');
    await transport.closeView('charts-1');
    expect(far.asked[1].appViewAction.surfaceId, 'charts-1');
    expect(far.asked[1].appViewAction.message, contains('action'));
    expect(far.asked[2].closeAppView.surfaceId, 'charts-1');

    await backend.close();
    await far.close();
  });
}
