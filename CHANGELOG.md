# Changelog

All notable changes to Media Buddy are tracked here.

## 0.1.0 - Public Preview

### Added

- Native Windows Tauri app with SolidJS frontend and Rust backend.
- Pixabay, Pexels, and Unsplash photo/video search.
- Persistent topics with per-provider cursor state.
- Batch downloads with local library storage and SQLite metadata.
- Large image and video inspector with editable captions and tags.
- Provider API key validation with save-on-valid behavior.
- Provider quota tracking based on observed API responses.
- Built-in REST API with bearer-token auth.
- Florence-2 ONNX model loading with GPU/CPU worker planning.
- Portable data layout beside the executable.
- Public README, wiki pages, PDF-ready manual, and GitHub templates.

### Notes

- Florence-2 model and runtime files are downloaded on first load and cached in
  `data/models/`.
- CUDA acceleration requires compatible NVIDIA drivers and CUDA 12 runtime DLLs
  available on the system path.
- The app is a public preview and should be tested with non-critical media
  libraries before large unattended jobs.
