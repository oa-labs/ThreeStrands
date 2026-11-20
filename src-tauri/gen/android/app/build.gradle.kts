import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

val verifierVersion = providers.fileContents(layout.projectDirectory.file("../../../Cargo.lock")).asText.map {
    Regex("""name = "rustls-platform-verifier-android"\s+version = "([^"]+)"""").find(it)
        ?.groupValues?.get(1) ?: error("Android TLS verifier is missing from Cargo.lock")
}

repositories {
    maven {
        url = uri("https://raw.githubusercontent.com/rustls/rustls-platform-verifier/maven-archive/android-release-support/maven/")
        content { includeGroup("org.rustls") }
    }
}

val uploadStore = providers.environmentVariable("THREESTRANDS_ANDROID_KEYSTORE")
val storeVersionCode = providers.environmentVariable("THREESTRANDS_ANDROID_VERSION_CODE")

android {
    signingConfigs {
        if (uploadStore.isPresent) {
            create("upload") {
                storeFile = file(uploadStore.get())
                storePassword = providers.environmentVariable("THREESTRANDS_ANDROID_STORE_PASSWORD").get()
                keyAlias = providers.environmentVariable("THREESTRANDS_ANDROID_KEY_ALIAS").get()
                keyPassword = providers.environmentVariable("THREESTRANDS_ANDROID_KEY_PASSWORD").get()
            }
        }
    }
    compileSdk = 36
    namespace = "app.threestrands.mail"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "app.threestrands.mail"
        minSdk = 28
        targetSdk = 36
        versionCode = storeVersionCode.orElse(tauriProperties.getProperty("tauri.android.versionCode", "1")).get().toInt()
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            signingConfig = signingConfigs.findByName("upload")
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation(verifierVersion.map { "org.rustls:rustls-platform-verifier:$it" })
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
