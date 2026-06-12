# Media Buddy 0.1.0 Release Notes

Media Buddy 0.1.0 is the first public preview release.

## Highlights

- Search Pixabay, Pexels, and Unsplash for photos and videos.
- Preview results before download.
- Save downloaded media into a local SQLite-backed library.
- Edit captions and tags.
- Track persistent search topics.
- Run batch downloads with concurrency controls.
- Use Florence-2 ONNX locally for image captions and object tags.
- Automate searches, downloads, metadata edits, and vision jobs with the local
  REST API.

## Recommended Download

Use the NSIS installer for normal Windows users:

```text
Media Buddy_0.1.0_x64-setup.exe
```

Portable testers can use:

```text
mediabuddy.exe
```

The portable exe creates a sibling `data/` folder on first run.

## Known Notes

- Florence-2 model/runtime files are not bundled. They download on first load
  and are cached under `data/models/`.
- CUDA acceleration requires compatible NVIDIA drivers and CUDA 12 runtime DLLs
  available on the system.
- Some provider video encodes may not preview in the WebView media stack.
- This is a public preview. Back up `data/` before large library changes.

## Artifact Paths

```text
src-tauri/target/release/mediabuddy.exe
src-tauri/target/release/bundle/nsis/Media Buddy_0.1.0_x64-setup.exe
src-tauri/target/release/bundle/msi/Media Buddy_0.1.0_x64_en-US.msi
```

## SHA256

Current verified build hashes:

```text
B5CE66DCBF2375AAE7F89DBE71ED944744A7539514A6C72F4F895B3AEC9F6E60  src-tauri\target\release\mediabuddy.exe
3161186724E8BAC7903924ABBAB921DA719E280417DBB22A0E919AD902EF5195  src-tauri\target\release\bundle\nsis\Media Buddy_0.1.0_x64-setup.exe
85D4DB897106B4441D7091D27F2135C75BF62E5C87014F985CFA31499309EA62  src-tauri\target\release\bundle\msi\Media Buddy_0.1.0_x64_en-US.msi
```

Regenerate before publishing if any source or build configuration changes:

```powershell
Get-FileHash "src-tauri\target\release\mediabuddy.exe" -Algorithm SHA256
Get-FileHash "src-tauri\target\release\bundle\nsis\Media Buddy_0.1.0_x64-setup.exe" -Algorithm SHA256
Get-FileHash "src-tauri\target\release\bundle\msi\Media Buddy_0.1.0_x64_en-US.msi" -Algorithm SHA256
```
