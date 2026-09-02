# Ausha

Real-time desktop audio streaming to receivers on the local network.
See `arch.md` for how the sender works and `plan.md` for the roadmap.

## Code style

**Clean code with good readability and maintainability.** This is the standing
rule for every change in this project, in every language.

- **The code expresses itself.** Use names that make the intent obvious and keep
  functions small enough to read in one pass. A reader should not need a comment
  to follow what a function does.
- **Minimum comments.** Write a comment only when the code genuinely cannot
  carry the information: a non-obvious *why*, a protocol or platform constraint,
  a deliberate trade-off. Never restate what the code already says, and never
  leave commented-out code behind.
- Prefer a one-line `//!` module doc stating the module's job over inline
  narration inside it.
- Keep modules focused on one responsibility, and keep platform-specific code
  behind `#[cfg]` in the module that owns that concern.
- Return errors rather than panicking on anything reachable at runtime;
  `expect` is for genuine invariants only.

## Checks before finishing

```bash
cargo fmt
cargo clippy --all-targets
cargo test
cargo build --release
```

Run from the workspace root. All four must be clean. Do not commit or push
unless asked.

For the Android app:

```bash
cd android && ./gradlew :app:assembleDebug
```

Gradle drives `cargo-ndk` itself, so never build the `.so` separately — a
hand-built library is how the Kotlin and the JNI signatures drift apart.

## Testing

`ausha-core` has no I/O, so its behaviour is tested headlessly: packets and
timestamps go in as arguments. Prefer a test that injects loss, reordering or
jitter into the core over one that needs a running sender. When a test needs a
network, drive it from a script rather than from the test binary.
