# The test APK of a release build (-PclaktoryTestBuildType=release): left
# as written, so JUnit and the AndroidX runner find their classes by name;
# only its references into the app are renamed, with the app's mapping.
-dontshrink
-dontoptimize
-dontobfuscate
-dontwarn **
