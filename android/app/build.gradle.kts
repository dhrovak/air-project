import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val rustRoot: File = rootProject.projectDir.parentFile
val cargoHome = File(System.getenv("CARGO_HOME") ?: "${System.getProperty("user.home")}/.cargo")
val cargo: String = File(cargoHome, "bin/cargo").takeIf { it.canExecute() }?.absolutePath ?: "cargo"
val rustAbis = listOf("arm64-v8a", "armeabi-v7a", "x86_64")
val rustMinSdk = 28
val jniLibsOut = layout.buildDirectory.dir("rust/jniLibs")
val bindingsOut = layout.buildDirectory.dir("generated/uniffi/kotlin")

// Release signing: android/keystore.properties (local) or SIGNING_* env vars (CI).
val keystoreProps = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.isFile) f.inputStream().use { load(it) }
}
fun signingValue(prop: String, env: String): String? =
    (keystoreProps.getProperty(prop) ?: System.getenv(env))?.takeIf { it.isNotBlank() }
val releaseStoreFile = signingValue("storeFile", "SIGNING_STORE_FILE")?.let { rootProject.file(it) }
val releaseStorePassword = signingValue("storePassword", "SIGNING_STORE_PASSWORD")
val releaseKeyAlias = signingValue("keyAlias", "SIGNING_KEY_ALIAS")
val releaseKeyPassword = signingValue("keyPassword", "SIGNING_KEY_PASSWORD")
val hasReleaseSigning = releaseStoreFile?.isFile == true &&
    releaseStorePassword != null && releaseKeyAlias != null && releaseKeyPassword != null

android {
    namespace = "com.dhrovak.airproject"
    compileSdk = 35
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "com.dhrovak.airproject"
        minSdk = rustMinSdk
        targetSdk = 35
        // Set by the release workflow; local builds report 1.0.
        versionCode = (findProperty("appVersionCode") as String?)?.toIntOrNull() ?: 1
        versionName = (findProperty("appVersionName") as String?) ?: "1.0"
        ndk { abiFilters += rustAbis }
    }

    signingConfigs {
        if (hasReleaseSigning) {
            create("release") {
                storeFile = releaseStoreFile
                storePassword = releaseStorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Falls back to the debug key so local release builds still install.
            signingConfig = signingConfigs.getByName(if (hasReleaseSigning) "release" else "debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    sourceSets["main"].jniLibs.srcDir(jniLibsOut)
    sourceSets["main"].java.srcDir(bindingsOut)
}

val cargoBuild by tasks.registering(Exec::class) {
    group = "rust"
    description = "Cross-compiles frame-core for all Android ABIs."
    workingDir = rustRoot
    val args = mutableListOf(cargo, "ndk", "--platform", rustMinSdk.toString())
    rustAbis.forEach { args += listOf("-t", it) }
    args += listOf("-o", jniLibsOut.get().asFile.absolutePath, "build", "--release", "-p", "frame-core", "--lib")
    commandLine(args)
    environment("PATH", "${File(cargoHome, "bin").absolutePath}:${System.getenv("PATH")}")
    doFirst { environment("ANDROID_NDK_HOME", android.ndkDirectory.absolutePath) }
    // Cargo does its own incremental tracking.
    outputs.upToDateWhen { false }
}

val generateBindings by tasks.registering(Exec::class) {
    group = "rust"
    description = "Generates Kotlin bindings for frame-core with uniffi-bindgen."
    dependsOn(cargoBuild)
    workingDir = rustRoot
    val lib = File(rustRoot, "target/aarch64-linux-android/release/libframe_core.so")
    commandLine(
        cargo, "run", "-q", "-p", "uniffi-bindgen", "--",
        "generate", "--library", lib.absolutePath,
        "--language", "kotlin", "--no-format",
        "--out-dir", bindingsOut.get().asFile.absolutePath,
    )
    environment("PATH", "${File(cargoHome, "bin").absolutePath}:${System.getenv("PATH")}")
    outputs.upToDateWhen { false }
}

tasks.matching { it.name == "preBuild" }.configureEach { dependsOn(generateBindings) }

base { archivesName.set("AirProject") }

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("androidx.preference:preference-ktx:1.2.1")
    implementation("androidx.work:work-runtime-ktx:2.9.1")
    implementation("androidx.media3:media3-exoplayer:1.4.1")
    implementation("androidx.media3:media3-ui:1.4.1")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
    implementation("net.java.dev.jna:jna:5.14.0@aar")
}
