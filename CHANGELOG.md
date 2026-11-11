# Changelog

All notable changes to Media Buddy are tracked here.

## 0.2.0 - 2026-11-10

### Added

- Captions record who wrote them (provider, AI, or typed by hand). The
  inspector shows this as **Caption by**; the REST API exposes it as
  `caption_source`.
- **Keep manual** option (on by default): AI runs never replace a caption typed
  by hand, in any caption mode. REST: `protect_manual`.
- Topic results stay with the topic until downloaded or dismissed, and come back
  after a restart or another search. New **Dismiss all** action and
  `GET`/`DELETE /api/v1/topics/{id}/pending`.
- **Settings -> Library -> Clear blocked list**, and `GET`/`DELETE
  /api/v1/blocked`, to let deleted items be found and downloaded again.
- Closing the window (or pressing Shutdown) while downloads or an analysis job
  are running asks for confirmation.
- REST file endpoints stream files and support `Range` requests.

### Fixed

- A caption or tags edited while an image was being analyzed could be
  overwritten when the result was saved. Results are now checked against the
  item as it is at save time.
- Unsaved text in the inspector was discarded when an AI result arrived.
- "AI captioned: yes" was shown for items whose caption was not AI-written. It
  is now **AI analyzed**, alongside **Caption by**.
- A failed provider call (rate limit, rejected key, outage) marked the topic
  source as exhausted for good. Failures are now reported and retried.
- A topic page whose results were all removed by "exclude AI" or a minimum
  size marked the source as exhausted.
- Adding tags to a selection could undo tag changes made since the library
  list was loaded.
- Unsplash photos were dropped when their detail call failed.
- Changing results-per-source between topic rounds skipped results.
- Multi-page REST searches returned duplicates instead of the last page.
- Videos were held fully in memory while downloading, and preview-only video
  downloads fetched the whole file. Videos now stream to disk.
- A video whose URL had no file extension could be saved with a `.jpg` name.
- An interrupted download could leave a truncated file in the library folder.
- The block on deleted items was partly forgotten after a restart.
- A photo and a video from the same provider that share a numeric id could hide
  each other.
- Unloading the model during an analysis job made the rest of that job fail.
- The REST file and analyze endpoints did not apply the data-folder containment
  check used by the desktop app.
- Finished REST tasks were kept in memory forever.
- The discovery file kept an old token after the token was changed.
- `PUT /api/v1/settings` accepted an empty token, an invalid host, and an
  unknown execution mode.
- The app could not start when installed to a folder it cannot write to; it now
  falls back to `%LOCALAPPDATA%\MediaBuddy\data`.
- Two settings saves at the same moment could overwrite each other.

### Changed

- The version shown in the app footer now follows the package version.

### Upgrade notes

- The library database is upgraded in place the first time this version opens
  it. Going back to 0.1.0 afterwards is not supported: 0.1.0 does not keep
  caption origins or topic positions up to date. Back up `data/` first if you
  may want to go back.
- Captions saved before this version show as "not recorded" and are treated as
  replaceable by AI runs, as before.

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
