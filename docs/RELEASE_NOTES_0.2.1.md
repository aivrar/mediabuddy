# Media Buddy 0.2.1 Release Notes

Media Buddy 0.2.1 closes a gap left by 0.2.0: captions saved by 0.1.0 have no
recorded origin, and 0.2.0 still let AI runs replace them. Some of those
captions were typed by hand. From this version they are protected unless you
say otherwise, and any caption an AI run replaces is kept so it can be put
back.

## Download

| File | Use it for |
| --- | --- |
| [Media.Buddy_0.2.1_x64-setup.exe](https://github.com/aivrar/mediabuddy/releases/download/v0.2.1/Media.Buddy_0.2.1_x64-setup.exe) | Installer. Recommended for most people. |
| [Media.Buddy_0.2.1_x64_en-US.msi](https://github.com/aivrar/mediabuddy/releases/download/v0.2.1/Media.Buddy_0.2.1_x64_en-US.msi) | MSI package, for environments that prefer MSI. |
| [mediabuddy.exe](https://github.com/aivrar/mediabuddy/releases/download/v0.2.1/mediabuddy.exe) | Portable. No install; keeps its data beside the exe. |

## Before You Upgrade

- Close Media Buddy and copy your `data/` folder somewhere safe.
- The library database is upgraded in place the first time 0.2.1 opens it,
  from 0.1.0 or 0.2.0. Going back to an older version afterwards is not
  supported.
- If you ran Replace short or Overwrite in 0.2.0 on captions saved by 0.1.0,
  the replaced text was not kept by that version and cannot be recovered.

## What Changed

### Captions of unknown origin are protected

- A caption whose origin is "not recorded" is no longer replaced by Replace
  short or Overwrite.
- A new option, **Keep unknown**, sits beside **Keep manual** and is on by
  default. Switch it off to let AI runs replace those captions too.
- Missing only and Tags only never replace an existing caption, as before.

### Replaced captions are kept

- When an AI run replaces a caption, the old text and its origin stay with the
  item.
- The inspector shows it as **Replaced caption**, with a **Restore** button.
- Restore swaps the two captions, so it can be undone and no text is lost.
- A second AI caption never pushes out a saved provider or hand-typed caption.

### Origins are recovered where they can be proven

The first time 0.2.1 opens a library, each caption with no recorded origin is
checked against the provider's own record for that item. No caption text is
changed.

- Still the provider's text: marked **provider**.
- Different, on an item that was never AI analyzed: marked **typed by hand**.
  Only a person can have changed it.
- Anything else stays **not recorded**. This is typically a caption on an item
  that was AI analyzed, which may be the AI's text or a later hand edit.

For a "not recorded" caption the inspector offers **Mark as mine**, which
records it as typed by hand without changing it.

### Clearer results

- The summary shown after a run says how many captions were kept because they
  were typed by hand or have no recorded origin.

## REST API

- New: `protect_unknown` on analysis requests, default `true`.
- **Behaviour change:** `overwrite` and `short` no longer replace captions with
  an empty `caption_source` unless `protect_unknown` is `false`.
- New: analysis results carry `caption_kept_reason` (`mode`, `manual`,
  `unknown_origin`, `tags_only`, `nothing_generated`, or null when written) and
  `caption_previous`.
- New: media rows carry `caption_previous` and `caption_previous_source`.
- New: `POST /api/v1/images/{id}/caption/restore`.
- New: `caption_source` on `PUT /api/v1/images/{id}`, `POST /api/v1/download`
  and `POST /api/v1/combo/download-analyze`, so a client can say who wrote the
  caption it sends. On `PUT` it can be sent alone to label an existing caption.

## Other Fixes

- Text typed in the inspector's tags box while a save was still in flight is no
  longer replaced when the save finishes.
- For a video saved without a poster, the library no longer loads the whole
  video file into memory as a thumbnail. It shows a plain tile.
- Tags a provider repeats are stored once.
- When the folder beside the exe cannot be used for data, no empty `data`
  folder tree is left there.

The full list is in [CHANGELOG.md](../CHANGELOG.md).

## How This Release Was Checked

- 53 automated tests, including the upgrade from a 0.1.0 database and from a
  0.2.0 database.
- A live run with Florence-2 against a copy of a real 0.1.0 library.
- Every changed screen exercised in the running app: the inspector, both
  protection options, Mark as mine, Restore, the run summary, Dismiss all, the
  blocked list in Settings, and the prompt shown when closing during a run.

## Known Notes

- The fallback to `%LOCALAPPDATA%\MediaBuddy\data` was tested by running the
  exe from a folder with write access denied. It has not been tested with a
  real MSI install under `C:\Program Files`.
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
60279DB6CE5352C68CAD47200B94DDB2742E896B4BF9560A151E63E8698EA91C  mediabuddy.exe
9B9B233A6CBCE06D5A4D824F22CAEED5544F7824D055C81453C87DAA3CD04B22  Media.Buddy_0.2.1_x64-setup.exe
56A35FD9AE60F16677D5FA037BCA67B86E5AE5FE0AC7C4596DE6D5D64CADA33F  Media.Buddy_0.2.1_x64_en-US.msi
```

Check a download with:

```powershell
Get-FileHash ".\Media.Buddy_0.2.1_x64-setup.exe" -Algorithm SHA256
```
