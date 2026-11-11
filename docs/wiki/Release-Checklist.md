# Release Checklist

## Before Building

- Run a clean git status review.
- Confirm no `data/`, logs, model cache, API keys, or downloaded media are
  staged.
- Update `CHANGELOG.md`.
- Update README screenshots.
- Confirm `src-tauri/tauri.conf.json`, `package.json`, `package-lock.json`,
  `src-tauri/Cargo.toml`, and `src-tauri/Cargo.lock` agree on version. The
  footer in the app reads its version from `package.json`.
- Add `docs/RELEASE_NOTES_<version>.md` and update the version, download
  links, and installer names in `README.md`, `SECURITY.md`,
  `docs/MEDIA_BUDDY_MANUAL.md`, and `docs/wiki/`.

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
src-tauri/target/release/bundle/nsis/Media Buddy_0.2.1_x64-setup.exe
src-tauri/target/release/bundle/msi/Media Buddy_0.2.1_x64_en-US.msi
src-tauri/target/release/mediabuddy.exe
```

GitHub replaces the spaces in uploaded file names with dots, so the download
links use:

```text
Media.Buddy_0.2.1_x64-setup.exe
Media.Buddy_0.2.1_x64_en-US.msi
```

## Hashes

Generate hashes:

```powershell
Get-FileHash "src-tauri\target\release\mediabuddy.exe" -Algorithm SHA256
Get-FileHash "src-tauri\target\release\bundle\nsis\Media Buddy_0.2.1_x64-setup.exe" -Algorithm SHA256
Get-FileHash "src-tauri\target\release\bundle\msi\Media Buddy_0.2.1_x64_en-US.msi" -Algorithm SHA256
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
- Confirm the README download links and release badge open the new release.
- Copy `docs/wiki/` to the GitHub Wiki repository.
