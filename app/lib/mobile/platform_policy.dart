import 'package:cupertino_ui/cupertino_ui.dart'
    show CupertinoPage, DefaultCupertinoLocalizations;
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';

import '../android/behavior_policy.dart';
import 'back.dart';
import 'input.dart';

/// Where a phone's behavior differs by platform, the one place that decides
/// it: how big a finger's target is, whether a swipe from the leading edge
/// opens the drawer, how a page is pushed, how text selects, and what
/// answers the system's Back. Everything else (the layouts, the words, the
/// look in docs/IDENTITY.md) is the same on every phone.
///
/// The iPhone's are the app's own; Android's are Android's
/// (android/behavior_policy.dart); a Mac's are the iPhone's, without the
/// native selection.
abstract class PhonePlatformPolicy {
  const PhonePlatformPolicy();

  /// The policy for the platform the app runs on.
  static PhonePlatformPolicy get current => forPlatform(defaultTargetPlatform);

  /// The policy for [platform].
  static PhonePlatformPolicy forPlatform(TargetPlatform platform) =>
      switch (platform) {
        TargetPlatform.iOS => const _IPhonePolicy(),
        TargetPlatform.android when debugAndroidPolicy =>
          const _AndroidPolicy(),
        _ => const _DesktopPolicy(),
      };

  /// Whether Android gets Android's policy (else a desktop's). flutter_test
  /// runs every test as Android unless it says otherwise, so the app's
  /// tests, written for the iPhone and a Mac, turn this off
  /// (test/flutter_test_config.dart); Android's own turn it back on.
  @visibleForTesting
  static bool debugAndroidPolicy = true;

  /// The policy where [context] is: [current].
  static PhonePlatformPolicy of(BuildContext context) => current;

  /// [height], or this platform's touch target where that asks for more
  /// than iOS's 44pt (Android's 48dp): a row the iPhone draws denser (the
  /// drawer's 36pt lines, its 44pt agents) keeps its height there, and a
  /// Mac's too.
  static double least(BuildContext context, double height) {
    final policy = of(context);
    return policy.touch &&
            policy.touchTarget > 44 &&
            policy.touchTarget > height
        ? policy.touchTarget
        : height;
  }

  /// Whether the platform is used with a finger (an iPhone, an iPad,
  /// Android) at any window size: [touchFirst].
  bool get touch;

  /// The least a tappable thing measures each way on a phone.
  double get touchTarget;

  /// Whether a swipe from the leading edge opens the drawer (else only
  /// Menu does: Android's edges are its Back).
  bool get drawerEdgeSwipe;

  /// A page pushed over the conversation.
  Page<T> page<T>({
    required LocalKey key,
    required Widget child,
    bool fullscreenDialog = false,
  });

  /// Whether text selects the platform's own way (handles, magnifier, the
  /// system's edit menu) rather than the desktop's mouse selection.
  bool get nativeSelection;

  /// How an [EditableText] selects: [nativeTextSelection].
  ({
    TextSelectionControls? controls,
    EditableTextContextMenuBuilder? contextMenuBuilder,
    TextMagnifierConfiguration magnifier,
  })
  selection(BuildContext context, {ExtraEditItems<EditableTextState>? extra});

  /// The edit menu of text that selects but doesn't edit:
  /// [nativeSelectableMenu].
  SelectableRegionContextMenuBuilder? selectableMenu({
    ExtraEditItems<SelectableRegionState>? extra,
  });

  /// The app's localizations: what its widgets' own words come from.
  List<LocalizationsDelegate<Object>> get localizations;

  /// Wraps the phone's screens (the drawer, the pages, under the sheets)
  /// in what answers the system's Back from [back]. None on an iPhone,
  /// which has no Back of its own.
  Widget backHost({required PhoneBack back, required Widget child});
}

class _IPhonePolicy extends PhonePlatformPolicy {
  const _IPhonePolicy();

  @override
  bool get touch => true;

  @override
  double get touchTarget => 44;

  @override
  bool get drawerEdgeSwipe => true;

  @override
  Page<T> page<T>({
    required LocalKey key,
    required Widget child,
    bool fullscreenDialog = false,
  }) => CupertinoPage<T>(
    key: key,
    fullscreenDialog: fullscreenDialog,
    child: child,
  );

  @override
  bool get nativeSelection => true;

  @override
  ({
    TextSelectionControls? controls,
    EditableTextContextMenuBuilder? contextMenuBuilder,
    TextMagnifierConfiguration magnifier,
  })
  selection(BuildContext context, {ExtraEditItems<EditableTextState>? extra}) =>
      iPhoneTextSelection(extra: extra);

  @override
  SelectableRegionContextMenuBuilder? selectableMenu({
    ExtraEditItems<SelectableRegionState>? extra,
  }) => iPhoneSelectableMenu(extra: extra);

  @override
  List<LocalizationsDelegate<Object>> get localizations => const [
    DefaultCupertinoLocalizations.delegate,
  ];

  @override
  Widget backHost({required PhoneBack back, required Widget child}) => child;
}

/// A Mac's (or any desktop's) narrow window: the iPhone's, with the
/// desktop's own mouse selection.
class _DesktopPolicy extends _IPhonePolicy {
  const _DesktopPolicy();

  @override
  bool get touch => false;

  @override
  bool get nativeSelection => false;

  @override
  ({
    TextSelectionControls? controls,
    EditableTextContextMenuBuilder? contextMenuBuilder,
    TextMagnifierConfiguration magnifier,
  })
  selection(BuildContext context, {ExtraEditItems<EditableTextState>? extra}) =>
      (
        controls: null,
        contextMenuBuilder: null,
        magnifier: TextMagnifierConfiguration.disabled,
      );

  @override
  SelectableRegionContextMenuBuilder? selectableMenu({
    ExtraEditItems<SelectableRegionState>? extra,
  }) => null;
}

/// Android's, from android/behavior_policy.dart.
class _AndroidPolicy extends PhonePlatformPolicy {
  const _AndroidPolicy();

  static const _android = AndroidBehaviorPolicy();

  @override
  bool get touch => true;

  @override
  double get touchTarget => _android.touchTarget;

  @override
  bool get drawerEdgeSwipe => _android.drawerEdgeSwipe;

  @override
  Page<T> page<T>({
    required LocalKey key,
    required Widget child,
    bool fullscreenDialog = false,
  }) => _android.page<T>(
    key: key,
    child: child,
    fullscreenDialog: fullscreenDialog,
  );

  @override
  bool get nativeSelection => true;

  @override
  ({
    TextSelectionControls? controls,
    EditableTextContextMenuBuilder? contextMenuBuilder,
    TextMagnifierConfiguration magnifier,
  })
  selection(BuildContext context, {ExtraEditItems<EditableTextState>? extra}) =>
      _android.selection(context, extra: extra);

  @override
  SelectableRegionContextMenuBuilder? selectableMenu({
    ExtraEditItems<SelectableRegionState>? extra,
  }) => _android.selectableMenu(extra: extra);

  @override
  List<LocalizationsDelegate<Object>> get localizations => [
    DefaultCupertinoLocalizations.delegate,
    ..._android.localizations,
  ];

  @override
  Widget backHost({required PhoneBack back, required Widget child}) =>
      _android.backHost(back: back, child: child);
}
