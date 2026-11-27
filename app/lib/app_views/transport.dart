import 'dart:async';

import 'package:flutter/foundation.dart';

/// App views: an app installed in Claktory shows
/// views the person opens in a pane (Mac) or a pushed screen (phone). The
/// backend holds each surface; the app here renders it with its own
/// widgets (catalog.dart) and sends back what the person does.
///
/// These types mirror the protocol's (OpenAppView, AppSurface,
/// AppViewUpdate, AppViewAction, CloseAppView) so the renderer doesn't
/// depend on how they travel: [AppViewTransport] is the backend's
/// connection in the app, a fake in tests.

/// The catalog this app renders: catalog v1's components, IDENTITY-styled.
const appViewCatalogV1 = 'https://claktory.com/a2ui/catalog/v1';

/// The catalogs every OpenAppView names.
const appViewCatalogs = [appViewCatalogV1];

/// Where a surface stands (AppSurface.state).
enum AppViewState {
  /// The app is making it.
  opening,

  /// Showing what the app sent.
  open,

  /// The app isn't running: it opens again by itself when it does.
  stopped,

  /// The app sent something invalid, or couldn't open it; the last good
  /// contents stay, under why.
  failed,

  /// The app closed it, or was removed.
  closed,

  /// This app can't render the surface's catalog: nothing was opened.
  unsupported,
}

/// A surface as the backend says it is now (AppSurface): replaces
/// everything held for it.
@immutable
class AppSurfaceData {
  const AppSurfaceData({
    required this.surfaceId,
    required this.appId,
    this.view = '',
    this.params = '{}',
    this.title = '',
    required this.state,
    this.error = '',
    this.catalog = appViewCatalogV1,
    this.seq = 0,
    this.snapshot = '',
  });

  final String surfaceId;
  final String appId;
  final String view;
  final String params;
  final String title;
  final AppViewState state;

  /// Why it's failed, stopped or closed, a sentence for the person.
  final String error;
  final String catalog;

  /// The last update folded into [snapshot].
  final int seq;

  /// Everything it shows now: one A2UI v1.0 createSurface message, JSON;
  /// empty before the app made it.
  final String snapshot;
}

/// One change to a surface's contents (AppViewUpdate): an A2UI v1.0
/// updateComponents, updateDataModel or deleteSurface message, JSON.
@immutable
class AppViewUpdateData {
  const AppViewUpdateData({
    required this.surfaceId,
    required this.seq,
    required this.message,
  });

  final String surfaceId;
  final int seq;
  final String message;
}

/// What the backend sends about the surfaces this connection opened.
sealed class AppViewEvent {
  const AppViewEvent();

  String get surfaceId;
}

/// The surface's state changed (an AppSurface sent unasked).
final class AppSurfaceChanged extends AppViewEvent {
  const AppSurfaceChanged(this.surface);

  final AppSurfaceData surface;

  @override
  String get surfaceId => surface.surfaceId;
}

/// Its contents changed.
final class AppSurfaceUpdated extends AppViewEvent {
  const AppSurfaceUpdated(this.update);

  final AppViewUpdateData update;

  @override
  String get surfaceId => update.surfaceId;
}

/// How app views reach the backend: the connection in the app, a fake in
/// tests. A refused request throws the backend's RequestFailure.
abstract interface class AppViewTransport {
  /// Opens [view] of [appId] with [params] (a JSON object, or empty) for
  /// this connection, answering the surface as it is now (OpenAppView).
  Future<AppSurfaceData> openView({
    required String appId,
    required String view,
    required String params,
    required List<String> catalogs,
  });

  /// Hands the app what the person did: one A2UI renderer→agent message,
  /// JSON (AppViewAction). Refused as not found when the surface is gone.
  Future<void> actInView(String surfaceId, String message);

  /// This connection no longer shows the surface (CloseAppView).
  Future<void> closeView(String surfaceId);

  /// Every AppSurface and AppViewUpdate for this connection's surfaces.
  Stream<AppViewEvent> get viewEvents;

  /// Whether the connection is up: surfaces live only in the backend's
  /// memory, so each is opened again once it's back.
  ValueListenable<bool> get viewsConnected;
}
