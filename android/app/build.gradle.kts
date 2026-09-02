import org.gradle.internal.os.OperatingSystem

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.compose.compiler)
}

android {
    namespace = "com.ausha.receiver"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.ausha.receiver"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "0.1"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures { compose = true }

    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("rustJniLibs"))
    sourceSets["main"].kotlin.srcDir("src/main/kotlin")
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.lifecycle.service)
    implementation(libs.androidx.media)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.compose.material3)
    implementation(libs.accompanist.permissions)
    implementation(libs.camera.camera2)
    implementation(libs.camera.lifecycle)
    implementation(libs.camera.view)
    implementation(libs.mlkit.barcode.scanning)
}

/**
 * Builds the Rust core for each Android ABI. Keeping this in the Gradle build
 * rather than a separate script means the .so can never be stale relative to
 * the Kotlin that calls into it.
 */
val abis = (findProperty("ausha.abis") as String? ?: "arm64-v8a,x86_64").split(",")

val cargoNdk = tasks.register<Exec>("cargoNdkBuild") {
    val out = layout.buildDirectory.dir("rustJniLibs").get().asFile
    val ndk = System.getenv("ANDROID_NDK_HOME")
        ?: "${System.getProperty("user.home")}/Android/Sdk/ndk/28.2.13676358"

    workingDir = rootProject.projectDir.parentFile
    environment("ANDROID_NDK_HOME", ndk)
    inputs.dir(File(rootProject.projectDir.parentFile, "core/src"))
    inputs.dir(File(rootProject.projectDir.parentFile, "client/src"))
    inputs.dir(File(rootProject.projectDir.parentFile, "mobile/src"))
    outputs.dir(out)

    val cargo = if (OperatingSystem.current().isWindows) "cargo.exe" else "cargo"
    commandLine(
        buildList {
            add(cargo)
            add("ndk")
            abis.forEach { add("-t"); add(it.trim()) }
            add("-o"); add(out.absolutePath)
            add("build"); add("--release"); add("-p"); add("ausha-mobile")
        }
    )
}

tasks.withType<com.android.build.gradle.tasks.MergeSourceSetFolders>().configureEach {
    dependsOn(cargoNdk)
}
