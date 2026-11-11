# Library And Downloads

## Downloads

Media Buddy stores downloaded originals and thumbnails locally. Batch downloads
run concurrently with controls in the Images tab.

For fastest practical use:

- Use a moderate concurrency value for large batches.
- Prefer provider topics when collecting many pages over time.
- Watch the log tab for provider errors or throttling.
- Lower concurrency if a provider starts rate-limiting requests.

Videos are written straight to disk as they download, so large files do not
take up memory. A preview-only video download saves the poster and metadata
without fetching the video file.

A file only gets its final name once it is complete. If the app is closed or
crashes mid-download, the unfinished `.part` file is removed at the next start.

Closing the window while downloads or an analysis job are running asks for
confirmation first.

## Library

Downloaded items appear in **Images -> Library**. You can filter by:

- Query.
- Caption.
- Author.
- Tags.
- Source.
- Kind.

## Inspector

The library inspector supports:

- Large image preview.
- Local video playback when the downloaded file is available.
- Previous/next navigation.
- Source metadata.
- Editable caption.
- Editable tags.
- Save and delete actions.

## Captions And Tags

Provider captions and tags are saved when available. AI vision can fill missing
data or supplement tags based on selected options.

The inspector shows who wrote each caption (**Caption by**: provider, AI, typed
by hand, or not recorded for items saved by older versions). Captions typed by
hand are kept by AI runs unless **Keep manual** is switched off. See
[AI Vision Florence-2](AI-Vision-Florence-2.md) for the full rules.

## Search Results Versus Library Items

Search result cards are not library items until downloaded. A saved topic can
exist even when no media from that topic has been downloaded yet.

## Deleting Media

Deleting a library item removes its database row and associated local media
files when available. Topic history can remain so the topic can continue search
pagination later.

A deleted item is also blocked: it no longer appears in search results and
cannot be downloaded again, including after a restart and when a provider lists
it under a different URL. To undo this, open **Settings -> Library** and press
**Clear blocked list**. That clears every blocked item at once.
