plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// The Cargo workspace owns the version number.
val workspaceVersion = Regex("""(?m)^version\s*=\s*"([^"]+)"""")
    .find(rootDir.resolve("../Cargo.toml").readText())
    ?.groupValues?.get(1)
    ?: error("workspace version is missing from Cargo.toml")

android {
    namespace = "com.q0id.q0editor"
    compileSdk = 35
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "com.q0id.q0editor"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = workspaceVersion

        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }

    // The activity base and theme q0player shares.
    sourceSets {
        getByName("main") {
            java.srcDir("../shared/java")
            res.srcDir("../shared/res")
        }
    }
}

dependencies {
    implementation("androidx.appcompat:appcompat:1.7.0")
    // Must match the GameActivity glue that the android-activity crate
    // compiles into the native library.
    implementation("androidx.games:games-activity:2.0.2")
}
