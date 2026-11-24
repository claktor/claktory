import 'package:flutter/widgets.dart';

/// The Android wide shell starts at 800 dp. In Sale's 635 dp tablet split,
/// Settings overflowed the composer by 105 px: it needs the phone's pages.
/// This is window width, not the tablet's physical display size.
const androidWideLayoutWidth = 800.0;

/// A narrow Android window uses the phone's drawer and in-place pages.
/// Keep the short-side guard too: turning a phone sideways must not select
/// the wide shell. Keyboard insets do not change this choice.
bool androidPhoneLayout(Size size, {required double phoneWidth}) =>
    size.width < androidWideLayoutWidth || size.shortestSide < phoneWidth;
