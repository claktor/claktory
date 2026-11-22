# The tunnel library's JNI entry binds by name to this class and method
# (app/rust/src/android.rs), so R8 must keep both as they are.
-keep class com.claktory.app.TunnelBootstrap {
    native <methods>;
}
