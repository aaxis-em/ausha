import org.gradle.internal.os.OperatingSystem
import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.compose.compiler)
}

/**
 * The ABIs the Rust core is cross-compiled for and the only ones packaged, so
 * the APK can never claim an ABI that has no `.so`. Narrow it for a faster
 * local build: `-Pausha.abis=arm64-v8a`.
 */
val abis = (findProperty("ausha.abis") as String? ?: "arm64-v8a,armeabi-v7a,x86_64")
    .split(",")
    .map { it.trim() }
    .filter { it.isNotEmpty() }

/** Pinned, because a different NDK produces a different `.so`. */
val pinnedNdkVersion = "28.2.13676358"

/**
 * Where the NDK lives. `ANDROID_NDK` is read as well because that is the name
 * F-Droid's build server exports.
 */
fun androidNdkHome(): String =
    System.getenv("ANDROID_NDK_HOME")
        ?: System.getenv("ANDROID_NDK")
        ?: findProperty("ausha.ndk") as String?
        ?: "${android.sdkDirectory}/ndk/$pinnedNdkVersion"

android {
    namespace = "com.ausha.receiver"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.ausha.receiver"
        minSdk = 26
        targetSdk = 34
        versionCode = 3
        versionName = "0.1.2"
        ndk { abiFilters += abis }
    }

    signingConfigs {
        create("release") { releaseKeystore()?.applyTo(this) }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            // F-Droid's build strips the signingConfigs block above out of this
            // file before running Gradle, so the lookup has to tolerate its
            // absence and stay on one line: a multi-line expression here is left
            // dangling by that edit and the script no longer compiles.
            signingConfig = signingConfigs.findByName("release")?.takeIf { it.storeFile != null }

            /** Only Play reads this, and it puts the git commit in the APK. */
            vcsInfo { include = false }
        }
    }

    /**
     * The Play dependency blob is encrypted and differs on every build, which
     * would defeat F-Droid's reproducible-build check.
     */
    dependenciesInfo {
        includeInApk = false
        includeInBundle = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
    buildFeatures { compose = true }

    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("rustJniLibs"))
    sourceSets["main"].kotlin.srcDir("src/main/kotlin")
    sourceSets["test"].kotlin.srcDir("src/test/kotlin")
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
    implementation(libs.zxing.core)
    testImplementation(libs.junit)
}

/**
 * Builds the Rust core for each Android ABI. Keeping this in the Gradle build
 * rather than a separate script means the .so can never be stale relative to
 * the Kotlin that calls into it.
 */
val cargoNdk = tasks.register<Exec>("cargoNdkBuild") {
    val out = layout.buildDirectory.dir("rustJniLibs").get().asFile
    val ndk = androidNdkHome()

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
            abis.forEach { add("-t"); add(it) }
            add("-o"); add(out.absolutePath)
            add("build"); add("--release"); add("-p"); add("ausha-mobile")
        }
    )
}

tasks.withType<com.android.build.gradle.tasks.MergeSourceSetFolders>().configureEach {
    dependsOn(cargoNdk)
}

data class ReleaseKeystore(
    val storeFile: File,
    val storePassword: String,
    val keyAlias: String,
    val keyPassword: String,
) {
    fun applyTo(config: com.android.build.api.dsl.ApkSigningConfig) {
        config.storeFile = storeFile
        config.storePassword = storePassword
        config.keyAlias = keyAlias
        config.keyPassword = keyPassword
    }
}

/**
 * Release signing material, kept out of the repository: either
 * `android/keystore.properties` or the four `AUSHA_KEYSTORE*` environment
 * variables. With neither present `assembleRelease` produces an unsigned APK,
 * which is all F-Droid needs — it signs with its own key.
 */
fun releaseKeystore(): ReleaseKeystore? {
    val file = rootProject.file("keystore.properties")
    val props = Properties().apply {
        if (file.exists()) file.inputStream().use { load(it) }
    }

    fun value(key: String, env: String) =
        props.getProperty(key) ?: System.getenv(env)

    val store = value("storeFile", "AUSHA_KEYSTORE")?.let(::File) ?: return null
    if (!store.exists()) {
        logger.warn("Release keystore $store does not exist; signing skipped.")
        return null
    }
    return ReleaseKeystore(
        storeFile = store,
        storePassword = value("storePassword", "AUSHA_KEYSTORE_PASSWORD") ?: return null,
        keyAlias = value("keyAlias", "AUSHA_KEY_ALIAS") ?: return null,
        keyPassword = value("keyPassword", "AUSHA_KEY_PASSWORD") ?: return null,
    )
}
