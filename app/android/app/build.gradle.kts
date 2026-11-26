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
/// never for anything that leaves this Mac). -PclaktoryKeyStore=FILE names
/// the keystore itself, over key.properties' storeFile: the vault hands
/// both files to a command at fresh temporary paths, so a storeFile written
/// in key.properties can't point at its keystore.
val keyProperties = Properties().apply {
    val file = providers.gradleProperty("claktoryKeyProperties").orNull?.let(::File)
        ?: rootProject.file("key.properties")
    require(file.exists() || file == rootProject.file("key.properties")) { "No key properties at $file" }
    if (file.exists()) file.inputStream().use { load(it) }
    providers.gradleProperty("claktoryKeyStore").orNull?.takeIf { it.isNotEmpty() }?.let {
        require(File(it).isFile) { "No keystore at $it" }
        setProperty("storeFile", it)
    }
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

    // Device tests run on debug builds; -PclaktoryTestBuildType=release runs
    // them on the shrunk release instead (with -PclaktoryDebugSignedRelease
    // locally), to catch what R8 breaks before a person does (DESIGN.md §10).
    testBuildType = providers.gradleProperty("claktoryTestBuildType").getOrElse("debug")

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
            // The native libraries' symbol tables, kept apart before they're
            // stripped: into the bundle for Play's crash reports, and as
            // native-debug-symbols.zip (release_apk.sh puts it beside the
            // .aab). The APK's libraries stay stripped.
            ndk { debugSymbolLevel = "SYMBOL_TABLE" }
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Device tests on the release build reach into the app's classes.
            if (testBuildType == "release") {
                proguardFiles("proguard-test-hooks.pro")
                testProguardFiles("proguard-test-apk.pro")
            }
        }
    }

    packaging {
        // Libraries load straight from the APK: stored, 16 KiB-aligned.
        jniLibs { useLegacyPackaging = false }
    }

    // AGP's dependency list, encrypted to Google, goes into the APK's
    // signing block afresh (and differently) on every build, so no two
    // APKs of one commit would match (DESIGN.md §6). An APK installed from
    // a link has no use for it; Play reads it from the bundle, which keeps it.
    dependenciesInfo { includeInApk = false }

    buildFeatures { resValues = true }

    // Fatal, so every release build stops on them (AGP runs lint's vital
    // checks before packaging): a call older Androids don't have (a launch
    // crash on Android 7 once) and a receiver other apps can reach. The
    // whole lint, errors and all, is tools/lint.sh, run before landing.
    lint {
        fatal += setOf("NewApi", "UnspecifiedRegisterReceiverFlag")
        abortOnError = true
        checkTestSources = true
    }
}

dependencies {
    // QR codes: reading the pairing code from the camera's frames and
    // drawing this device's own (QrScanner.kt). ZXing, Apache-2.0, pure
    // Java, no transitive dependencies; licence checked by Fugo. Exact
    // version, from Maven Central only (../build.gradle.kts).
    // core-3.5.3.jar SHA-256 8d8064c1636fdaef7189dd9055c7d59950a8940a12f2293956446ec3c109fd82
    // (its SHA-1, ca1349214a356cd7958651b2d5a0e1f3811a9c4b, is Maven Central's published one).
    implementation("com.google.zxing:core:3.5.3")
    // Custom Tabs with a session for shared websites (ShareBrowser.kt, W6):
    // the version url_launcher_android already puts in every build, whose
    // dependencies (androidx.core and the rest) come with it.
    implementation("androidx.browser:browser:1.9.0") { isTransitive = false }
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

// The scheme links open the app with (factory/links, WebLinks.kt), as on
// the Mac: claktory for Claktory, claktory-dev for Claktory Dev, and
// claktory-debug for a debug build, so claktory:// always opens the real
// Claktory.
androidComponents {
    onVariants { variant ->
        val scheme = when {
            variant.flavorName == "dev" -> "claktory-dev"
            variant.buildType == "debug" -> "claktory-debug"
            else -> "claktory"
        }
        variant.manifestPlaceholders.put("linkScheme", scheme)
        variant.resValues.put(
            variant.makeResValueKey("string", "link_scheme"),
            com.android.build.api.variant.ResValue(scheme),
        )
    }
}
