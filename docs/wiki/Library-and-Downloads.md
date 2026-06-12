# Library And Downloads

## Downloads

Media Buddy stores downloaded originals and thumbnails locally. Batch downloads
run concurrently with controls in the Images tab.

For fastest practical use:

- Use a moderate concurrency value for large batches.
- Prefer provider topics when collecting many pages over time.
- Watch the log tab for provider errors or throttling.
- Lower concurrency if a provider starts rate-limiting requests.

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
data or supplement tags based on selected options. Use caption overwrite options
carefully when provider data is already useful.

## Search Results Versus Library Items

Search result cards are not library items until downloaded. A saved topic can
exist even when no media from that topic has been downloaded yet.

## Deleting Media

Deleting a library item removes its database row and associated local media
files when available. Topic history can remain so the topic can continue search
pagination later.
