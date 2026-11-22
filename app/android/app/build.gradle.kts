import java.util.Properties

plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

/// The pinned build tuple (../toolchain.properties, DESIGN.md §1).
val pins = Properties().apply {
    rootProject.file("toolchain.properties").inputStream().use { load(it) }
}
fun pin(key: String): String = requireNotNull(pins.getProperty(key)) { "toolchain.properties has no $key" }

/// The release signing key, when this machine has one: key.properties
/// (storeFile, storePassword, keyAlias, keyPassword; never committed), here
/// or wherever -PclaktoryKeyProperties=FILE says, so the key can live
/// outside the checkout. Without it a release build fails, unless it's asked to
/// be signed with the debug key for a local test (-PclaktoryDebugSignedRelease=true,
/// never for anything that leaves this Mac).
val keyProperties = Properties().apply {
    val file = providers.gradleProperty("claktoryKeyProperties").orNull?.let(::File)
        ?: rootProject.file("key.properties")
    require(file.exists() || file == rootProject.file("key.properties")) { "No key properties at $file" }
    if (file.exists()) file.inputStream().use { load(it) }
}
val debugSignedRelease = providers.gradleProperty("claktoryDebugSignedRelease").orNull == "true"

android {
    namespace = "com.claktory.app"
    compileSdk = pin("android.compileSdk").toInt()
    buildToolsVersion = pin("android.buildTools")
    ndkVersion = pin("android.ndk")

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        applicationId = "com.claktory.app"
        minSdk = pin("android.minSdk").toInt()
        targetSdk = pin("android.targetSdk").toInt()
        versionCode = flutter.versionCode
        versionName = flutter.versionName
        // 64-bit only: no ABI is advertised without its tunnel library.
        ndk { abiFilters += pin("android.abis").split(",") }
        resValue("string", "app_name", "Claktory")
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    // Claktory and Claktory Dev, side by side, as on the iPhone; scripts
    // pass --flavor dev together with --dart-define=FACTORY_DEV=true.
    flavorDimensions += "channel"
    productFlavors {
        create("prod") {
            dimension = "channel"
        }
        create("dev") {
            dimension = "channel"
            applicationIdSuffix = ".dev"
            resValue("string", "app_name", "Claktory Dev")
        }
    }

    signingConfigs {
        if (keyProperties.isNotEmpty()) {
            create("release") {
                storeFile = rootProject.file(keyProperties.getProperty("storeFile"))
                storePassword = keyProperties.getProperty("storePassword")
                keyAlias = keyProperties.getProperty("keyAlias")
                keyPassword = keyProperties.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            signingConfig = when {
                keyProperties.isNotEmpty() -> signingConfigs.getByName("release")
                debugSignedRelease -> signingConfigs.getByName("debug")
                else -> null
            }
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    packaging {
        // Libraries load straight from the APK: stored, 16 KiB-aligned.
        jniLibs { useLegacyPackaging = false }
    }

    buildFeatures { resValues = true }
}

dependencies {
    testImplementation("junit:junit:4.13.2")
    // Android's org.json is a stub in local unit tests; the stores' tests
    // run against the reference implementation of the same API.
    testImplementation("org.json:json:20250517")
    // The Keystore store's tests on a phone or emulator (DESIGN.md §5),
    // at the versions the integration_test plugin already puts in debug builds.
    androidTestImplementation("androidx.test:runner:1.3.0")
    androidTestImplementation("androidx.test.ext:junit:1.1.2")
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}
