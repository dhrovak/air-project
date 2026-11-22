# AirProject

**A Rust-based screensaver for your moments.**

Turn an Android TV, Google TV or projector into a digital photo frame for your **iCloud Shared Albums**. No Apple account is needed on the TV. It works with any shared album that has **Public Website** turned on.

## Install

1. Download `AirProject.apk` from the [latest release](../../releases/latest).
2. Sideload it on the TV, for example with `adb install AirProject.apk` or an app like *Downloader* or *Send Files to TV*.
3. Open **AirProject** from the home screen and follow the on-screen setup.

The app needs Android 9 (API 28) or newer on arm64, armv7 or x86_64.

### Sharing an album

In Photos on iPhone or Mac, open the shared album and turn on **Public Website** in its settings. Then tap **Share › Copy Link** and paste the link into AirProject.

### Using it as the screensaver

Google TV often hides third-party screensavers. If AirProject doesn't show up in the system screensaver settings, enable developer options and run:

```sh
adb connect <tv-ip>
adb shell settings put secure screensaver_components com.dhrovak.airproject/.PhotoDreamService
adb shell settings put secure screensaver_enabled 1
```

To let the app open the slideshow by itself after boot on TVs that have no screen for this permission:

```sh
adb shell appops set com.dhrovak.airproject SYSTEM_ALERT_WINDOW allow
```

## Building from source

Requirements:

- Rust (stable) with the Android targets:
  `rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android`
- [`cargo-ndk`](https://github.com/bbqsrc/cargo-ndk): `cargo install cargo-ndk`
- JDK 17
- Android SDK with NDK `27.2.12479018`, either via `ANDROID_HOME` or `android/local.properties` (`sdk.dir=...`)

```sh
# Rust tests
cargo test -p frame-core

# Try syncing an album on your computer
cargo run -p frame-core --bin album-cli -- <shared-album-url> --out album-cache

# Debug APK (Gradle cross-compiles the Rust library and generates bindings automatically)
cd android
./gradlew assembleDebug
```

The APK is written to `android/app/build/outputs/apk/debug/AirProject-debug.apk`.

### Signed release builds

Without signing settings, `assembleRelease` signs with the local debug key. To sign with your own key, create `android/keystore.properties`. This file is git-ignored.

```properties
storeFile=/absolute/path/to/release.jks
storePassword=...
keyAlias=...
keyPassword=...
```

Instead of the file, you can set the environment variables `SIGNING_STORE_FILE`, `SIGNING_STORE_PASSWORD`, `SIGNING_KEY_ALIAS` and `SIGNING_KEY_PASSWORD`.

## Releasing

Releases are automatic. [The release workflow](.github/workflows/release.yml) runs the tests, builds the APK and publishes a GitHub Release:

- **AirProject v1.0** is created on the first push to `main`.
- Each pull request merged into `main` then creates the next minor version: v1.1, v1.2, and so on.
- Direct pushes to `main` that don't come from a pull request don't create a release. To release one anyway, run the workflow manually from the Actions tab.

The version name and code are set automatically, so you never edit them in `build.gradle.kts`. For v1.N, the version name is `1.N` and the version code is `N + 1`.

To sign with a stable key, add these repository secrets. Without a stable key, users can't install updates over an older version.

| Secret | Value |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | `base64 -w0 release.jks` |
| `ANDROID_KEYSTORE_PASSWORD` | Keystore password |
| `ANDROID_KEY_ALIAS` | Key alias |
| `ANDROID_KEY_PASSWORD` | Key password |

## Disclaimer

This project is not affiliated with or endorsed by Apple. It uses the same public endpoints as the iCloud shared album web viewer, and those endpoints may change at any time.

## License

Copyright © 2026 Dhruv Bhat

AirProject is free software: you can redistribute it and/or modify it under the terms of the [GNU General Public License](LICENSE) as published by the Free Software Foundation, either version 3 of the License, or (at your option) any later version.
