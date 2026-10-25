## What does this change?

<!-- A short description, and the issue it fixes if there is one. -->

## How was it tested?

<!-- Include commands and results. Identify any hardware qualification still outstanding. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo test --locked --workspace --exclude wor-desktop`
- [ ] `cargo clippy --locked --workspace --all-targets -- -D warnings`
- [ ] `npm run check`
- [ ] No physical media was written by automated tests

## Checklist

- [ ] Shared behavior remains in Rust domain/application/backend crates
- [ ] CLI and desktop remain consistent
- [ ] README and runtime inventory are updated when behavior or payloads change
- [ ] Attribution and third-party notices are preserved
- [ ] Any remaining hardware qualification is stated explicitly
- [ ] Sent applicable upstream changes to Botspot/wor-flasher
