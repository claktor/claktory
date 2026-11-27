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

/// The ABIs packaged: toolchain.properties' android.abis, or a part of them
/// that -PclaktoryAbis=arm64-v8a names (a build on an Apple-silicon Mac).
val pinnedAbis = pin("android.abis").split(",")
val shippedAbis = providers.gradleProperty("claktoryAbis").orNull
    ?.split(",")?.map { it.trim() }?.filter { it.isNotEmpty() }
    ?.also { asked -> require(asked.isNotEmpty() && asked.all { it in pinnedAbis }) { "claktoryAbis must name some of $pinnedAbis, not $asked" } }
    ?: pinnedAbis

/// Nothing is built for x86 on an Apple-silicon Mac (Matt's rule, Bruno
/// 48603/48619): Flutter compiles the app, the Rust tunnel included, for
/// every platform in -Ptarget-platform (all of them, android-x64 too, when
/// it's not given), so a build here that would compile android-x64 stops
/// before it does. Builds here pass --target-platform android-arm64; a
/// release with x86_64 is built on prime (DESIGN.md, "Where builds run").
val appleSilicon = System.getProperty("os.name").orEmpty().startsWith("Mac") && System.getProperty("os.arch") == "aarch64"
val flutterTargets = providers.gradleProperty("target-platform").orNull?.split(",")?.map { it.trim() }
    ?: listOf("android-arm", "android-arm64", "android-x64")
val compilesX86 = "android-x64" in flutterTargets

/// Firebase's Android app configs (google-services.json, one per app:
/// com.claktory.app, com.claktory.app.dev), for notifications through FCM
/// (PushChannel.kt): -PclaktoryFirebaseConfig=FILE[,FILE] or
/// CLAKTORY_FIREBASE_CONFIG, never committed (the API key in them would
/// stop the secrets check; the vault hands them to the build). Each
/// variant gets the values Firebase starts from (google_app_id and the
/// rest, as Google's plugin would write them) from the client for its
/// application id; without one it has none, Firebase doesn't start, and
/// the phone reads as getting no notifications (DESIGN.md, Notifications).
val firebaseClients: Map<String, Map<String, String>> = (
    providers.gradleProperty("claktoryFirebaseConfig").orNull
        ?: providers.environmentVariable("CLAKTORY_FIREBASE_CONFIG").orNull
    ).orEmpty().split(",").map { it.trim() }.filter { it.isNotEmpty() }.flatMap { path ->
        val file = File(path)
        require(file.isFile) { "No Firebase config at $path" }
        @Suppress("UNCHECKED_CAST")
        val json = groovy.json.JsonSlurper().parse(file) as Map<String, Any?>
        val project = json["project_info"] as? Map<String, Any?> ?: emptyMap()
        (json["client"] as? List<Map<String, Any?>> ?: emptyList()).mapNotNull { client ->
            val info = client["client_info"] as? Map<String, Any?> ?: return@mapNotNull null
            val packageName = (info["android_client_info"] as? Map<String, Any?>)?.get("package_name") as? String
                ?: return@mapNotNull null
            val apiKey = (client["api_key"] as? List<Map<String, Any?>>)?.firstOrNull()?.get("current_key") as? String
            packageName to listOfNotNull(
                (info["mobilesdk_app_id"] as? String)?.let { "google_app_id" to it },
                (project["project_number"] as? String)?.let { "gcm_defaultSenderId" to it },
                apiKey?.let { "google_api_key" to it },
                (project["project_id"] as? String)?.let { "project_id" to it },
                (project["storage_bucket"] as? String)?.let { "google_storage_bucket" to it },
            ).toMap()
        }
    }.toMap()

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
        ndk { abiFilters += shippedAbis }
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
        jniLibs {
            useLegacyPackaging = false
            // Only the ABIs shipped, whatever a library brings: Flutter's
            // plugin adds its own default ABI filters to a release build,
            // so a dependency's native library (Firebase's datastore) came
            // in for 32-bit ARM too, and Play would offer the app to phones
            // it can't run on. check_artifact.py refuses one as well.
            excludes += listOf("armeabi-v7a", "arm64-v8a", "x86", "x86_64", "armeabi", "mips", "mips64")
                .filter { it !in shippedAbis }.map { "/lib/$it/**" }
        }
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
    // Notifications through Firebase Cloud Messaging (PushChannel.kt,
    // ClaktoryMessagingService.kt): the token, and the pushes Claktory's
    // relay sends, sealed end to end. Exact version, from Google's Maven;
    // it brings Firebase's common parts and Google Play services' messaging
    // client with it. Started only once the person allows notifications.
    implementation("com.google.firebase:firebase-messaging:26.0.2")
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
        // Firebase's start values for this variant's app, when the build was given them.
        val applicationId = if (variant.flavorName == "dev") "com.claktory.app.dev" else "com.claktory.app"
        val firebase = firebaseClients[applicationId]
        if (firebaseClients.isNotEmpty() && firebase == null) {
            logger.warn("No Firebase config for $applicationId: ${variant.name} gets no notifications.")
        }
        firebase?.forEach { (name, value) ->
            variant.resValues.put(
                variant.makeResValueKey("string", name),
                com.android.build.api.variant.ResValue(value),
            )
        }
    }
}

// The stop for x86 on this kind of Mac, at the step that would compile it
// (Flutter's, which runs the Rust build too), so configuring, lint and
// unit tests that compile nothing for a platform aren't stopped.
if (appleSilicon && compilesX86) {
    tasks.matching { it.name.startsWith("compileFlutterBuild") }.configureEach {
        doFirst {
            throw GradleException(
                "This build would compile for android-x64 (x86_64), and nothing is built for x86 on an " +
                    "Apple-silicon Mac. Build arm64 only here (--target-platform android-arm64), and build " +
                    "a release with x86_64 on prime (app/android/DESIGN.md, \"Where builds run\").",
            )
        }
    }
}
