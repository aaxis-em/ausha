# Publishing the Android app on F-Droid

F-Droid does not build from this directory. It builds from its own repository,
`fdroiddata`, using a recipe that has to be submitted there. This directory
holds our copy of that recipe so it is reviewed alongside the code it builds,
plus the steps to get it in and keep it current.

| File | What it is |
|---|---|
| `com.ausha.receiver.yml` | The build recipe. A copy of `metadata/com.ausha.receiver.yml` in fdroiddata. |

The app listing itself — name, descriptions, icon, changelogs, screenshots —
does **not** live in the recipe. F-Droid reads it from
`fastlane/metadata/android/` at the repository root on every build, so those
files are edited here and never in fdroiddata.

## What makes this app buildable by F-Droid

Five things, each of which is easy to break:

1. **No proprietary dependencies.** F-Droid's scanner rejects the build if it
   finds one. QR scanning is `com.google.zxing:core`, not ML Kit. Before adding
   any dependency, check its licence.
2. **A licence in the repository.** `LICENSE`, GPL-3.0-or-later, matching the
   `License:` field in the recipe.
3. **A pinned, reproducible toolchain.** `rust-toolchain.toml` pins the Rust
   compiler and `pinnedNdkVersion` in `android/app/build.gradle.kts` pins the
   NDK. The recipe installs exactly those versions.
4. **No Play dependency blob.** `dependenciesInfo { includeInApk = false }`,
   because that blob is encrypted and differs on every build.
5. **A build script that survives having its signing config deleted.** Before
   running Gradle, `fdroid build` edits `build.gradle.kts` in place: it deletes
   the whole `signingConfigs { ... }` block and every line matching
   `signingConfig = <no-spaces>`. Anything left dangling by those two deletions
   is a compile error, which is why the `signingConfig` assignment is a single
   line using `findByName`. To check a change to that part of the file, run the
   real edit against a clean copy before tagging — the regexes are
   `gradle_signing_configs` and `gradle_line_matches` in fdroidserver's
   `common.py`.

## Cutting a release

1. Bump `versionCode` and `versionName` in `android/app/build.gradle.kts`.
   `versionCode` must increase by at least one; `versionName` is what users see.
2. Write `fastlane/metadata/android/en-US/changelogs/<versionCode>.txt`. The
   file name is the **version code**, not the version name.
3. Commit, then tag: `git tag -s v<versionName> && git push --tags`. The recipe's
   `UpdateCheckMode: Tags ^v[0-9.]+$` looks for exactly this shape, and a signed
   tag is what lets F-Droid trust the commit it builds.
4. Confirm the tag builds from a clean checkout (below).
5. For the first release only, open a merge request against
   [fdroiddata](https://gitlab.com/fdroid/fdroiddata) adding
   `metadata/com.ausha.receiver.yml` — a copy of the file next to this README.
   Run `fdroid lint com.ausha.receiver` and `fdroid rewritemeta
   com.ausha.receiver` in that checkout first; both must come back clean.
   `rewritemeta` drops comments, which is why everything a reviewer needs is in
   the `MaintainerNotes` field rather than in a comment. If it reflows lines it
   has no reason to touch, the local `ruamel.yaml` is folding long scalars
   differently from the one in fdroiddata's CI image: keep CI's wrapping and
   change only the lines that carry meaning, or the `rewritemeta` job fails.
   Lint does not check the metadata schema, so run that too — it is a separate
   CI job and it rejects fields lint accepts:

   ```bash
   check-jsonschema --schemafile schemas/metadata.json metadata/com.ausha.receiver.yml
   ```

   After that, `AutoUpdateMode: Version` picks up new tags on its own and no
   further merge requests are needed unless the recipe itself changes.

If the recipe does need changing — a new NDK, a new Rust version, a new system
package — change it **here and in fdroiddata in the same release**. The copy in
this directory is worthless the moment it drifts.

## Checking a tag builds the way F-Droid will build it

Reproduce the build server's conditions: a clean checkout, no `local.properties`,
nothing cached from your working tree.

```bash
git clone --branch v<versionName> https://github.com/aaxis-em/ausha.git /tmp/ausha-check
cd /tmp/ausha-check/android
ANDROID_HOME=$HOME/Android/Sdk ./gradlew clean :app:assembleRelease
```

The APK lands in `android/app/build/outputs/apk/release/`. Verify it carries a
`.so` for each of the three ABIs and nothing else:

```bash
unzip -l app/build/outputs/apk/release/*.apk | grep '\.so$'
```

With `fdroid` installed you can also run the recipe itself, which is the only
way to test the `sudo:` block:

```bash
fdroid build --server com.ausha.receiver:<versionCode>
```

## Screenshots

F-Droid shows whatever is in
`fastlane/metadata/android/en-US/images/phoneScreenshots/`, sorted by file name,
so name them in the order they should be read: `1_pairing.png`,
`2_playing.png`, `3_call_mode.png`. PNG or JPEG at the device's own resolution,
no device frame and no added captions. Anything that is not an image is ignored,
which is why the directory holds a `.gitkeep`.

Worth showing, in this order: the QR pairing screen, playback running with the
latency and loss statistics visible, and call mode switched on.

## Signing

F-Droid signs the APKs it distributes with its own key, so an unsigned release
build is enough for F-Droid. Releases published anywhere else — GitHub, a
direct download — must be signed with our key, which the Gradle build picks up
from `android/keystore.properties` (git-ignored) or the environment:

```properties
storeFile=/absolute/path/to/ausha-release.jks
storePassword=...
keyAlias=ausha
keyPassword=...
```

or `AUSHA_KEYSTORE`, `AUSHA_KEYSTORE_PASSWORD`, `AUSHA_KEY_ALIAS` and
`AUSHA_KEY_PASSWORD`. With neither present the release APK is simply unsigned.

Keep the keystore and its passwords off this repository and backed up
somewhere you will still have in five years. Losing it means every existing
install has to be uninstalled before it can be updated.
