# Search And Topics

## Search Sources

Media Buddy can search:

- Pixabay
- Pexels
- Unsplash

Searches can include photos, videos, or one kind at a time depending on the
selected controls.

## Result Counts

The result count is the requested provider result budget. A query can return
fewer results when a provider has fewer matches, quota is exhausted, or the
provider filters out unsafe/unavailable items.

## Topics

Topics are saved search plans. They remember:

- Query.
- Media kind.
- Enabled providers.
- Per-provider cursor/page state.
- Saved library items touched by that topic.

The topic list shows values such as:

```text
0/63 PHOTO
```

The left number is how many items from that topic are saved in the library. The
right number is how many results have been collected or are currently known for
that topic.

## Getting More Results

Use a topic's **More** behavior to continue from its saved provider cursor
instead of starting over. This helps avoid downloading the same items repeatedly
when gathering large sets.

## Search Preview

Search results are not automatically saved. They are provider results until you
download them. Use the inspector or double-click preview to check an item before
download.

## Quota Awareness

Provider quotas are shown from observed responses. Some providers expose clear
remaining quota headers and some expose less detail. Treat the quota panel as a
useful snapshot, not a guaranteed provider contract.
