import 'package:flutter_rust_bridge_hooks/flutter_rust_bridge_hooks.dart';

/// Builds the private tunnel's Rust crate (rust/) into the app, through
/// Flutter's native assets: no CocoaPods or Xcode changes, the same for iOS
/// later.
void main(List<String> args) async {
  await build(args, (input, output) async {
    await const FlutterRustBridgeNativeAssetsBuilder(
      cratePath: 'rust',
      assetName: 'tunnel/frb_generated.io.dart',
    ).run(input: input, output: output);
  });
}
