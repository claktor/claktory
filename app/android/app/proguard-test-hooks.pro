# Only for device tests on a release build (-PclaktoryTestBuildType=release),
# never in an APK that leaves this Mac. The tests reach into the app's own
# classes (test hooks, constructors with defaults, constants R8 would fold
# away), so nothing of ours is removed; R8 still renames and optimizes it
# all, and the test APK is renamed with the app's mapping, so what renaming
# and shrinking break (JNI names, manifest components, reflection) still
# shows.
-keep,allowobfuscation class com.claktory.app.** { *; }
# The tests call Kotlin's standard library too, which lives in the app's
# APK: keep it whole there (still renamed).
-keep,allowobfuscation class kotlin.** { *; }
# PlaybackTest makes its own engine, which the app never does.
-keep,allowobfuscation class io.flutter.embedding.engine.FlutterEngine { *; }
