import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

/// The system's share sheet, for sending text (a pipe's invite code, say)
/// through Messages, Mail or any app: on an iPhone or an iPad, through
/// `factory/device`'s `share` (AppDelegate.swift). Elsewhere there's none
/// yet: offer Copy alone where [canShare] is false.
const _device = MethodChannel('factory/device');

/// Whether this device has a share sheet the app can open.
bool canShare(BuildContext context) =>
    !kIsWeb && defaultTargetPlatform == TargetPlatform.iOS;

/// Opens the share sheet for [text]. On an iPad it points at [from], the
/// Share button's rect in global coordinates. True once the person sent
/// it somewhere; false if they closed it, or there's no sheet here.
Future<bool> shareText(BuildContext context, String text, {Rect? from}) async {
  if (!canShare(context) || text.isEmpty) return false;
  try {
    return await _device.invokeMethod<bool>('share', {
          'text': text,
          if (from != null)
            'rect': [from.left, from.top, from.width, from.height],
        }) ??
        false;
  } on MissingPluginException {
    return false;
  } on PlatformException {
    return false;
  }
}

/// [context]'s widget's rect in global coordinates, for [shareText]'s
/// `from`; null before it's laid out.
Rect? globalRectOf(BuildContext context) {
  final box = context.findRenderObject();
  if (box is! RenderBox || !box.hasSize) return null;
  return box.localToGlobal(Offset.zero) & box.size;
}
