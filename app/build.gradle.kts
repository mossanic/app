import org.gradle.internal.os.OperatingSystem

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "org.mossanic.app"
    compileSdk = 34

    defaultConfig {
        applicationId = "org.mossanic.app"
        minSdk = 26
        targetSdk = 28 // Critical for exec() later
        versionCode = 1
        versionName = "0.1.0"

        ndk {
            abiFilters.add("arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    kotlinOptions {
        jvmTarget = "1.8"
    }
    sourceSets["main"].jniLibs.srcDirs("src/main/jniLibs")
}

// Build Rust before assembling the app
val buildRust = tasks.register<Exec>("buildRust") {
    workingDir = file("${rootDir}/rust")
    commandLine("cargo", "ndk", "-t", "arm64-v8a", "-o", "../app/src/main/jniLibs", "build", "--release")
}

tasks.named("preBuild") {
    dependsOn(buildRust)
}
