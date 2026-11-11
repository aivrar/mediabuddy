# Troubleshooting

## App Does Not Start

- Install Microsoft Edge WebView2 Runtime.
- Try the NSIS installer instead of portable mode.
- Check Windows Defender or antivirus quarantine.
- Run from a simple path such as `C:\Tools\MediaBuddy\`.

## API Key Test Fails

- Confirm the key is pasted into the matching provider field.
- Retry once after a short delay.
- Check provider quota and account status.
- Check the Log tab for provider-specific errors.

## Search Returns No Results

- Confirm at least one provider is enabled.
- Confirm the key for that provider is valid.
- Try a simpler query.
- Reduce filters such as orientation or safe-search behavior.
- Check quota.

## A Topic Shows A Provider Error

- Read the reason shown with the error: rate limit reached, key rejected, or
  the provider is having problems.
- Wait and run the topic again. The failed page is retried; nothing is skipped.
- A source is only marked exhausted when the provider really has no more
  results. Reset the topic to start it over.

## A Deleted Item Never Comes Back In Search

- Deleted items are blocked on purpose, including after a restart.
- Use **Settings -> Library -> Clear blocked list** to allow them again.

## Download Is Slow Or Fails

- Lower download concurrency if providers throttle requests.
- Try one provider at a time.
- Check available disk space.
- Check logs for HTTP status codes.

## Video Does Not Play

- Confirm the item is downloaded, not only a search result.
- Try another provider video.
- Check that the original file exists under `data/videos/originals/`.
- Some provider encodes may not be supported by the WebView media stack.

## Florence-2 Does Not Load

- Check internet access for first-time downloads.
- Try CPU mode to separate model download issues from GPU provider issues.
- For CUDA, verify NVIDIA driver and CUDA 12 runtime DLLs.
- For DirectML, verify the GPU and driver support the required DirectML feature
  level.
- Delete `data/models/` and load again if integrity checks fail.

## Florence-2 Analyze Fails

- Ensure the selected item is an image.
- Lower worker count.
- Restart the app after switching runtime families.
- Try CPU mode as a baseline.
- Check the Log tab and GitHub issue template for useful report details.

## AI Did Not Replace A Caption

- Read the summary shown when the run finishes: it counts the captions kept
  because they were typed by hand or have no recorded origin.
- Check **Caption by** in the inspector. A caption typed by hand is kept while
  **Keep manual** is on; a "not recorded" caption is kept while **Keep
  unknown** is on.
- Check the caption mode: Missing only and Replace short leave longer captions
  alone.

## AI Replaced A Caption I Wanted

- Open the item. The old text is under **Replaced caption**; press
  **Restore**.

## The Window Asks Before Closing

- A download or an analysis job is still running. Let it finish, or confirm to
  quit anyway.

## REST API Requests Fail

- Open the API tab and confirm the server is running.
- Use the current token from the API tab.
- Include `Authorization: Bearer <token>` for non-status endpoints.
- Confirm the host and port match Settings.
- HTTP 409 from a download means the URL was deleted from the library and is
  blocked. HTTP 409 from vision unload means an analysis job is running.
- HTTP 502 from a single-source search means the provider call failed; the
  message carries the provider's status.
