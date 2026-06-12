# Release Checklist

## Before Building

- Run a clean git status review.
- Confirm no `data/`, logs, model cache, API keys, or downloaded media are
  staged.
- Update `CHANGELOG.md`.
- Update README screenshots.
- Confirm `src-tauri/tauri.conf.json`, `package.json`, and
  `src-tauri/Cargo.toml` agree on version.

## Validation

```powershell
npm run build
cargo test --manifest-path src-tauri\Cargo.toml
cargo clippy --manifest-path src-tauri\Cargo.toml --all-targets -- -D warnings
git diff --check
```

## Package

```powershell
npm run tauri build -- --bundles nsis,msi
```

## Release Artifacts

Attach:

```text
src-tauri/target/release/bundle/nsis/Media Buddy_0.1.0_x64-setup.exe
src-tauri/target/release/bundle/msi/Media Buddy_0.1.0_x64_en-US.msi
src-tauri/target/release/mediabuddy.exe
```

## Hashes

Generate hashes:

```powershell
Get-FileHash "src-tauri\target\release\mediabuddy.exe" -Algorithm SHA256
Get-FileHash "src-tauri\target\release\bundle\nsis\Media Buddy_0.1.0_x64-setup.exe" -Algorithm SHA256
Get-FileHash "src-tauri\target\release\bundle\msi\Media Buddy_0.1.0_x64_en-US.msi" -Algorithm SHA256
```

## GitHub Release Notes

Include:

- Short app summary.
- What's new.
- Known limitations.
- Installation notes.
- Portable mode notes.
- SHA256 hashes.
- Link to manual and wiki.

## After Publishing

- Download the published installer from GitHub and launch it.
- Confirm first-run Settings flow.
- Confirm API docs open.
- Confirm screenshots render in README.
