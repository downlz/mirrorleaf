# Add project specific ProGuard rules here.
# You can control the set of applied configuration files using the
# proguardFiles setting in build.gradle.
#
# For more details, see
#   http://developer.android.com/guide/developing/tools/proguard.html

# If your project uses WebView with JS, uncomment the following
# and specify the fully qualified class name to the JavaScript interface
# class:
#-keepclassmembers class fqcn.of.javascript.interface.for.webview {
#   public *;
#}

# Uncomment this to preserve the line number information for
# debugging stack traces.
#-keepattributes SourceFile,LineNumberTable

# If you keep the line number information, uncomment this to
# hide the original source file name.
#-renamesourcefileattribute SourceFile

# Mirrorleaf (ADR-005). Release builds shrink code, and these are reached only
# from native code, which the shrinker can't see.
# JNA, which the engine's uniffi bindings use to call the native library.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-dontwarn java.awt.**
# The engine's generated bindings, and the startup call.
-keep class app.mirrorleaf.engine.** { *; }
-keep class app.mirrorleaf.MirrorleafNative { *; }
# rustls-platform-verifier's Kotlin part, called through JNI.
-keep, includedescriptorclasses class org.rustls.platformverifier.** { *; }
