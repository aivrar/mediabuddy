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

You can change the results-per-source value between rounds. The topic tracks how
many items it has consumed from each provider, so a different page size never
skips results.

## Results Wait For You

Results a topic has fetched stay with the topic until you download or dismiss
them. If you close the app, switch tabs, or run another search, they come back
the next time you open or search that topic, marked as waiting from earlier
rounds.

**Dismiss all** removes the results on screen (and anything else still waiting
for that topic) without downloading them. Dismissed results are not offered
again unless you reset the topic.

## Provider Errors

A provider call that fails (rate limit reached, key rejected, provider outage)
is reported as an error with the reason. It does not mark the source as
exhausted: the same page is tried again on the next round, and nothing is
skipped. A source is only marked exhausted when the provider really returns no
more results.

Unsplash's extra per-photo detail calls are optional enrichment. If one fails,
the photo is still shown using its search summary.

## Search Preview

Search results are not automatically saved. They are provider results until you
download them. Use the inspector or double-click preview to check an item before
download.

## Quota Awareness

Provider quotas are shown from observed responses. Some providers expose clear
remaining quota headers and some expose less detail. Treat the quota panel as a
useful snapshot, not a guaranteed provider contract.
