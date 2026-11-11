# Media Buddy 0.2.0 Release Notes

Media Buddy 0.2.0 is a reliability release. It fixes a set of problems where
the app could lose or overwrite your work: captions and tags replaced by AI
runs, search results skipped or lost, and downloads left half-written.

Media Buddy is a Windows desktop app for searching, downloading, organizing,
previewing, and AI-captioning stock photos and videos from Pixabay, Pexels, and
Unsplash.

## Download

| File | Use it for |
| --- | --- |
| [Media.Buddy_0.2.0_x64-setup.exe](https://github.com/aivrar/mediabuddy/releases/download/v0.2.0/Media.Buddy_0.2.0_x64-setup.exe) | Installer. Recommended for most people. |
| [Media.Buddy_0.2.0_x64_en-US.msi](https://github.com/aivrar/mediabuddy/releases/download/v0.2.0/Media.Buddy_0.2.0_x64_en-US.msi) | MSI package, for environments that prefer MSI. |
| [mediabuddy.exe](https://github.com/aivrar/mediabuddy/releases/download/v0.2.0/mediabuddy.exe) | Portable. No install; keeps its data beside the exe. |

## Before You Upgrade

- Close Media Buddy and copy your `data/` folder somewhere safe.
- The library database is upgraded in place the first time 0.2.0 opens it.
  Going back to 0.1.0 afterwards is not supported.
- Captions saved by 0.1.0 show as "not recorded" under **Caption by**, because
  0.1.0 did not track who wrote them. AI runs treat them as before. Captions
  you edit from now on are recorded as typed by hand and protected.

## Captions And Tags

- **Caption origin.** Every caption records who wrote it: the provider, the AI,
  or you. The inspector shows it as **Caption by**; the REST API returns it as
  `caption_source`.
- **Keep manual.** A new option, on by default, stops AI runs from replacing a
  caption typed by hand, in every caption mode including Overwrite. REST:
  `protect_manual`.
- **Editing during an AI run is safe.** A caption or tags saved while an image
  is being analyzed are kept. Each result is checked against the item as it is
  when the result is saved, and AI tags are added on top of the current tags.
- **Unsaved typing is kept.** Text typed in the inspector is no longer wiped
  when an AI result arrives.
- **Clearer status.** "AI captioned: yes" is now **AI analyzed**, shown beside
  **Caption by**. An item can be analyzed without its caption being AI-written.
- Adding tags to a selection no longer undoes tag changes made since the
  library list was loaded.

## Search And Topics

- **Provider errors no longer end a topic.** A rate limit, a rejected key, or a
  provider outage used to mark the source as exhausted for good. It is now
  reported with its reason, and the same page is retried next time.
- **Results wait for you.** Results a topic has fetched stay with it until you
  download or dismiss them, across restarts and other searches. **Dismiss all**
  clears them.
- Changing results-per-source between rounds no longer skips results.
- A page whose results were all removed by "exclude AI" or a minimum size no
  longer marks the source as exhausted.
- Unsplash photos are no longer dropped when their detail call fails.
- A photo and a video from the same provider that share an id no longer hide
  each other.

## Downloads And Library

- Videos stream straight to disk instead of being held in memory. Preview-only
  video downloads no longer fetch the whole file.
- A file only gets its final name once it is complete. Unfinished files left by
  a crash are removed at the next start.
- Closing the window, or pressing Shutdown, while downloads or an AI run are in
  progress asks first.
- Deleted items stay blocked after a restart. **Settings -> Library -> Clear
  blocked list** lets them be found and downloaded again.
- A video whose URL has no file extension is no longer saved with a `.jpg`
  name.

## AI Vision

- The model cannot be unloaded while an analysis job is using it. Before, that
  made the rest of the job fail.

## REST API

- New: `GET` / `DELETE /api/v1/blocked`.
- New: `GET` / `DELETE /api/v1/topics/{topic_id}/pending`.
- File and thumbnail endpoints stream the file and support `Range` requests.
- File and analyze endpoints refuse stored paths outside the data folder.
- `PUT /api/v1/settings` rejects an empty token, a host that is not an IP
  address, and an unknown execution mode.
- Single-source searches return HTTP 502 when the provider call fails, instead
  of an empty result.
- Multi-page searches no longer return duplicates in place of the last page.
- Finished tasks are pruned (the most recent 200, for six hours).
- The discovery file follows token changes straight away.

## Other Fixes

- The app starts when installed to a folder it cannot write to; data then goes
  to `%LOCALAPPDATA%\MediaBuddy\data`.
- Two settings saves at the same moment can no longer overwrite each other.
- The version shown in the app footer follows the package version.

The full list is in [CHANGELOG.md](../CHANGELOG.md).

## Known Notes

- The fallback data folder for installs under `C:\Program Files` has not been
  tested with a real MSI install.
- Florence-2 model/runtime files are not bundled. They download on first load
  and are cached under `data/models/`.
- CUDA acceleration requires compatible NVIDIA drivers and CUDA 12 runtime DLLs
  available on the system.
- Some provider video encodes may not preview in the WebView media stack.
- This is a public preview. Back up `data/` before large library changes.

## Documentation

- Wiki: https://github.com/aivrar/mediabuddy/wiki
- Manual: [MEDIA_BUDDY_MANUAL.md](MEDIA_BUDDY_MANUAL.md)

## SHA256

```text
902CB7AF5958E25592B74BE3A4F064B547ED0FFCF30D809F3B0B23EF4C3AB642  mediabuddy.exe
EC342B9553FB2319F527616BCEA786F83DB337FA4ADD5B2684E68A51CF3479AB  Media.Buddy_0.2.0_x64-setup.exe
405B53D302A81D05678866784969C3A5636C44229F5FC4B908FB8EB13E23193E  Media.Buddy_0.2.0_x64_en-US.msi
```

Check a download with:

```powershell
Get-FileHash ".\Media.Buddy_0.2.0_x64-setup.exe" -Algorithm SHA256
```
