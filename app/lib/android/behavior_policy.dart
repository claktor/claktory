import 'package:material_ui/material_ui.dart'
    show
        AdaptiveTextSelectionToolbar,
        DefaultMaterialLocalizations,
        FadeForwardsPageTransitionsBuilder,
        MaterialRouteTransitionMixin,
        TextMagnifier,
        materialTextSelectionHandleControls;
import 'package:flutter/widgets.dart';

import '../mobile/back.dart';
import '../mobile/input.dart' show ExtraEditItems;
import 'back_host.dart';

/// Android implementations for the shared PhonePlatformPolicy seam.
/// Not a shell or an alternative set of product screens. Binding is owned by
/// mobile/platform_policy.dart; these helpers do nothing until selected there.
final class AndroidBehaviorPolicy {
  const AndroidBehaviorPolicy();

  /// Applies to touch input at every window width, not just phone layouts.
  double get touchTarget => 48;

  /// Android owns both system Back edges. Menu remains the drawer opener.
  bool get drawerEdgeSwipe => false;

  /// Android route transitions are driven by our single Back host, not a
  /// second native predictive observer that could also pop under an overlay.
  /// This adds no Material scaffold, typography or product chrome.
  Page<T> page<T>({
    required LocalKey key,
    required Widget child,
    bool fullscreenDialog = false,
  }) => _AndroidPage<T>(
    key: key,
    fullscreenDialog: fullscreenDialog,
    child: child,
  );

  Widget backHost({required PhoneBack back, required Widget child}) =>
      AndroidBackHost<BackTarget>(
        changes: back,
        readTarget: () => back.target,
        canClose: (target) => target.canClose,
        commitTarget: (target) => back.commit(target),
        pageKey: (target) =>
            target is PageTarget ? ValueKey<String>(target.id) : null,
        child: child,
      );

  /// English fallback for the existing WidgetsApp. Full locale support needs
  /// GlobalMaterial/Widgets/CupertinoLocalizations via the shared app owner;
  /// this fallback alone is explicitly not a localization acceptance pass.
  List<LocalizationsDelegate<Object>> get localizations => const [
    DefaultMaterialLocalizations.delegate,
  ];

  ({
    TextSelectionControls? controls,
    EditableTextContextMenuBuilder? contextMenuBuilder,
    TextMagnifierConfiguration magnifier,
  })
  selection(BuildContext context, {ExtraEditItems<EditableTextState>? extra}) =>
      (
        controls: materialTextSelectionHandleControls,
        contextMenuBuilder: (context, state) => _SelectionBackSurface(
          back: PhoneBack.maybeOf(state.context),
          onBack: state.hideToolbar,
          child: AdaptiveTextSelectionToolbar.buttonItems(
            anchors: state.contextMenuAnchors,
            buttonItems: [
              ...state.contextMenuButtonItems,
              ...?extra?.call(state),
            ],
          ),
        ),
        magnifier: TextMagnifier.adaptiveMagnifierConfiguration,
      );

  SelectableRegionContextMenuBuilder selectableMenu({
    ExtraEditItems<SelectableRegionState>? extra,
  }) =>
      (context, state) => _SelectionBackSurface(
        back: PhoneBack.maybeOf(state.context),
        onBack: state.hideToolbar,
        child: AdaptiveTextSelectionToolbar.buttonItems(
          anchors: state.contextMenuAnchors,
          buttonItems: [
            ...state.contextMenuButtonItems,
            ...?extra?.call(state),
          ],
        ),
      );
}

/// The host owns the predictive transaction. Flutter 3.50 broadcasts predictive
/// events to every observer, so the standard Material predictive observer must
/// not also commit a Navigator.pop independently of the shared token registry.
class _AndroidPage<T> extends Page<T> {
  const _AndroidPage({
    required LocalKey super.key,
    required this.child,
    this.fullscreenDialog = false,
  });
  final Widget child;
  final bool fullscreenDialog;

  @override
  Route<T> createRoute(BuildContext context) => _AndroidPageRoute<T>(this);
}

class _AndroidPageRoute<T> extends PageRoute<T>
    with MaterialRouteTransitionMixin<T> {
  _AndroidPageRoute(_AndroidPage<T> page)
    : super(settings: page, fullscreenDialog: page.fullscreenDialog);

  @override
  bool get maintainState => true;

  // Disable the SDK's independent predictive observer even if a future theme
  // delegates to its builder. The host drives the route explicitly and owns
  // eligibility through NavigationNotification, not this native-gesture flag.
  @override
  bool get popGestureEnabled => false;

  @override
  DelegatedTransitionBuilder? get delegatedTransition => null;

  @override
  Widget buildContent(BuildContext context) {
    // Navigator updates settings when a Pages entry changes. Never retain the
    // constructor's child, which would leave a same-key page showing old data.
    final page = settings as _AndroidPage<T>;
    return AndroidBackPageMotion(pageKey: page.key!, child: page.child);
  }

  @override
  Widget buildTransitions(
    BuildContext context,
    Animation<double> animation,
    Animation<double> secondaryAnimation,
    Widget child,
  ) {
    if (MediaQuery.disableAnimationsOf(context)) return child;
    return const FadeForwardsPageTransitionsBuilder().buildTransitions<T>(
      this,
      context,
      animation,
      secondaryAnimation,
      child,
    );
  }
}

/// The selection toolbar is a Back surface even though Flutter paints it in
/// another overlay. Its registry comes from the field/region's context, not
/// whichever overlay happens to own its pixels. No registration on desktop/iOS.
class _SelectionBackSurface extends StatefulWidget {
  const _SelectionBackSurface({
    required this.back,
    required this.onBack,
    required this.child,
  });
  final PhoneBack? back;
  final VoidCallback onBack;
  final Widget child;

  @override
  State<_SelectionBackSurface> createState() => _SelectionBackSurfaceState();
}

class _SelectionBackSurfaceState extends State<_SelectionBackSurface> {
  late final _token = _SelectionBackToken(() => widget.onBack());
  BackHandle? _handle;

  @override
  void initState() {
    super.initState();
    _handle = widget.back?.register(_token);
  }

  @override
  void didUpdateWidget(_SelectionBackSurface oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.back != widget.back) {
      _handle?.release();
      _handle = widget.back?.register(_token);
    }
  }

  @override
  void dispose() {
    _handle?.release();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.child;
}

class _SelectionBackToken implements BackToken {
  const _SelectionBackToken(this._back);
  final VoidCallback _back;

  @override
  bool get canClose => true;

  @override
  void back() => _back();
}
