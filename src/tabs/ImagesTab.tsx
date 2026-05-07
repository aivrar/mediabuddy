import {
  createMemo,
  createSignal,
  For,
  onMount,
  Show,
  type Component,
} from "solid-js";
import {
  api,
  type Image,
  type SearchKindParam,
  type SearchResult,
  type SourcePages,
} from "../lib/api";
import { dropThumb } from "../lib/thumbCache";
import LibraryThumb from "../components/LibraryThumb";
import "./ImagesTab.css";

type Mode = "search" | "library";

const formatDuration = (sec: number | null) => {
  if (sec == null) return "";
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return `${m}:${String(s).padStart(2, "0")}`;
};

const formatStat = (n: number | null | undefined) => {
  if (n == null) return null;
  if (n >= 1000000) return `${(n / 1000000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
  return String(n);
};

const ImagesTab: Component = () => {
  const [mode, setMode] = createSignal<Mode>("library");

  // ---- Library state ----
  const [images, setImages] = createSignal<Image[]>([]);
  const [libLoading, setLibLoading] = createSignal(false);
  const [libError, setLibError] = createSignal<string | null>(null);
  const [librarySelection, setLibrarySelection] = createSignal(new Set<string>());
  const [filterText, setFilterText] = createSignal("");
  const [filterSource, setFilterSource] = createSignal<string>("");
  const [filterKind, setFilterKind] = createSignal<string>("");
  const [sortField, setSortField] = createSignal<"downloaded_at" | "width" | "source">(
    "downloaded_at"
  );
  const [sortDesc, setSortDesc] = createSignal(true);
  const [busy, setBusy] = createSignal(false);

  const refreshLibrary = async () => {
    setLibLoading(true);
    setLibError(null);
    try {
      setImages(await api.listImages());
    } catch (e) {
      setLibError(String(e));
    } finally {
      setLibLoading(false);
    }
  };
  onMount(refreshLibrary);

  const filteredImages = createMemo(() => {
    let list = images();
    const text = filterText().trim().toLowerCase();
    if (text) {
      list = list.filter(
        (i) =>
          i.query.toLowerCase().includes(text) ||
          i.alt.toLowerCase().includes(text) ||
          i.author_name.toLowerCase().includes(text) ||
          i.tags.some((t) => t.toLowerCase().includes(text))
      );
    }
    const src = filterSource();
    if (src) list = list.filter((i) => i.source === src);
    const kind = filterKind();
    if (kind) list = list.filter((i) => i.kind === kind);
    const f = sortField();
    const desc = sortDesc();
    list = [...list].sort((a, b) => {
      let cmp: number;
      if (f === "width") cmp = a.width - b.width;
      else if (f === "source") cmp = a.source.localeCompare(b.source);
      else cmp = a.downloaded_at.localeCompare(b.downloaded_at);
      return desc ? -cmp : cmp;
    });
    return list;
  });

  const sources = createMemo(() => {
    const set = new Set<string>();
    for (const i of images()) set.add(i.source);
    return Array.from(set).sort();
  });

  const kinds = createMemo(() => {
    const set = new Set<string>();
    for (const i of images()) set.add(i.kind);
    return Array.from(set).sort();
  });

  const toggleLibrarySel = (id: string) => {
    const sel = new Set(librarySelection());
    if (sel.has(id)) sel.delete(id);
    else sel.add(id);
    setLibrarySelection(sel);
  };
  const selectAllLibrary = () => {
    setLibrarySelection(new Set(filteredImages().map((i) => i.id)));
  };
  const clearLibrarySel = () => setLibrarySelection(new Set());

  const deleteSelected = async () => {
    const ids = Array.from(librarySelection());
    if (!ids.length) return;
    if (
      !confirm(
        `Delete ${ids.length} item${ids.length === 1 ? "" : "s"}? URLs will be blocked from re-downloading.`
      )
    )
      return;
    setBusy(true);
    try {
      const res = await api.deleteImages(ids);
      for (const id of ids) dropThumb(id);
      clearLibrarySel();
      await refreshLibrary();
      console.log(`Deleted ${res.deleted}, failed ${res.failed}`);
    } catch (e) {
      setLibError(String(e));
    } finally {
      setBusy(false);
    }
  };

  // ---- Search state ----
  const [query, setQuery] = createSignal("");
  const [pixabayPages, setPixabayPages] = createSignal(1);
  const [pexelsPages, setPexelsPages] = createSignal(1);
  const [unsplashPages, setUnsplashPages] = createSignal(1);
  const [searchKind, setSearchKind] = createSignal<SearchKindParam>("photo");
  const [results, setResults] = createSignal<SearchResult[]>([]);
  const [resultsSelection, setResultsSelection] = createSignal(new Set<string>());
  const [searching, setSearching] = createSignal(false);
  const [searchInfo, setSearchInfo] = createSignal<string | null>(null);
  const [searchError, setSearchError] = createSignal<string | null>(null);
  const [downloadInfo, setDownloadInfo] = createSignal<string | null>(null);

  const onSearch = async (e?: Event) => {
    e?.preventDefault();
    const q = query().trim();
    if (!q) return;
    setSearching(true);
    setSearchError(null);
    setSearchInfo(null);
    setDownloadInfo(null);
    setResults([]);
    setResultsSelection(new Set());
    const sources: SourcePages = {};
    if (pixabayPages() > 0) sources.pixabay = pixabayPages();
    if (pexelsPages() > 0) sources.pexels = pexelsPages();
    if (unsplashPages() > 0) sources.unsplash = unsplashPages();
    try {
      const t0 = performance.now();
      const r = await api.searchImages(q, sources, searchKind());
      const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
      setResults(r);
      const photos = r.filter((x) => x.kind === "photo").length;
      const videos = r.filter((x) => x.kind === "video").length;
      const breakdown =
        searchKind() === "both"
          ? ` (${photos} photo${photos === 1 ? "" : "s"} + ${videos} video${videos === 1 ? "" : "s"})`
          : "";
      setSearchInfo(
        `${r.length} new result${r.length === 1 ? "" : "s"}${breakdown} for "${q}" in ${elapsed}s`
      );
    } catch (e) {
      setSearchError(String(e));
    } finally {
      setSearching(false);
    }
  };

  const toggleResultSel = (url: string) => {
    const sel = new Set(resultsSelection());
    if (sel.has(url)) sel.delete(url);
    else sel.add(url);
    setResultsSelection(sel);
  };
  const selectAllResults = () => {
    setResultsSelection(new Set(results().map((r) => r.url)));
  };
  const clearResultsSel = () => setResultsSelection(new Set());

  const downloadSelected = async () => {
    const sel = resultsSelection();
    const items = results().filter((r) => sel.has(r.url));
    if (!items.length) return;
    setBusy(true);
    setDownloadInfo(`Downloading ${items.length}...`);
    try {
      const t0 = performance.now();
      const saved = await api.downloadImages(items, { concurrency: 8 });
      const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
      setDownloadInfo(
        `Saved ${saved.length} of ${items.length} in ${elapsed}s. Switch to Library to see them.`
      );
      const savedUrls = new Set(saved.map((s) => s.url));
      setResults(results().filter((r) => !savedUrls.has(r.url)));
      clearResultsSel();
      await refreshLibrary();
    } catch (e) {
      setDownloadInfo(`Error: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  const downloadAll = async () => {
    if (!results().length) return;
    setBusy(true);
    setDownloadInfo(`Downloading ${results().length}...`);
    try {
      const items = results();
      const t0 = performance.now();
      const saved = await api.downloadImages(items, { concurrency: 8 });
      const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
      setDownloadInfo(`Saved ${saved.length} of ${items.length} in ${elapsed}s.`);
      const savedUrls = new Set(saved.map((s) => s.url));
      setResults(results().filter((r) => !savedUrls.has(r.url)));
      await refreshLibrary();
    } catch (e) {
      setDownloadInfo(`Error: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="images-tab">
      <header class="images-header">
        <div class="mode-switch">
          <button
            class="mode-btn"
            classList={{ active: mode() === "library" }}
            onClick={() => setMode("library")}
            type="button"
          >
            Library
            <span class="mode-count">{images().length}</span>
          </button>
          <button
            class="mode-btn"
            classList={{ active: mode() === "search" }}
            onClick={() => setMode("search")}
            type="button"
          >
            Search
            <Show when={results().length > 0}>
              <span class="mode-count">{results().length}</span>
            </Show>
          </button>
        </div>
      </header>

      <Show when={mode() === "search"}>
        <SearchBar
          query={query()}
          onQueryChange={setQuery}
          pixabayPages={pixabayPages()}
          pexelsPages={pexelsPages()}
          unsplashPages={unsplashPages()}
          onPixabayChange={setPixabayPages}
          onPexelsChange={setPexelsPages}
          onUnsplashChange={setUnsplashPages}
          kind={searchKind()}
          onKindChange={setSearchKind}
          searching={searching()}
          onSearch={onSearch}
        />

        <Show when={searchInfo()}>
          <div class="info-banner">{searchInfo()}</div>
        </Show>
        <Show when={downloadInfo()}>
          <div class="info-banner">{downloadInfo()}</div>
        </Show>
        <Show when={searchError()}>
          <div class="error-banner">Error: {searchError()}</div>
        </Show>

        <Show when={results().length > 0}>
          <div class="batch-bar">
            <div class="batch-info">
              {resultsSelection().size} of {results().length} selected
            </div>
            <div class="batch-actions">
              <button type="button" onClick={selectAllResults} disabled={busy()}>
                Select all
              </button>
              <button type="button" onClick={clearResultsSel} disabled={busy()}>
                Clear
              </button>
              <button
                type="button"
                class="primary"
                onClick={downloadSelected}
                disabled={busy() || resultsSelection().size === 0}
              >
                Download selected
              </button>
              <button
                type="button"
                onClick={downloadAll}
                disabled={busy() || results().length === 0}
              >
                Download all
              </button>
            </div>
          </div>

          <div class="grid">
            <For each={results()}>
              {(r) => {
                const selected = () => resultsSelection().has(r.url);
                const previewUrl = () =>
                  r.urls?.poster ||
                  r.urls?.webformat ||
                  r.urls?.medium ||
                  r.urls?.regular ||
                  r.urls?.small ||
                  r.url ||
                  "";
                return (
                  <div
                    class="card"
                    classList={{ selected: selected(), [`kind-${r.kind}`]: true }}
                    onClick={() => toggleResultSel(r.url)}
                  >
                    <div class="card-image-wrap">
                      <img
                        src={previewUrl()}
                        alt={r.alt || r.query}
                        loading="lazy"
                      />
                      <Show when={r.kind === "video"}>
                        <span class="kind-badge video-badge">▶ {formatDuration(r.duration_secs)}</span>
                      </Show>
                      <Show when={r.kind !== "video" && r.kind !== "photo"}>
                        <span class="kind-badge">{r.kind}</span>
                      </Show>
                    </div>
                    <div class="card-meta">
                      <span class="card-source">{r.source}</span>
                      <Show when={r.author_name}>
                        <span class="card-author" title={r.author_name}>
                          {r.author_name}
                        </span>
                      </Show>
                    </div>
                    <Show when={r.likes != null || r.views != null}>
                      <div class="card-stats">
                        <Show when={formatStat(r.views)}>
                          <span title="views">👁 {formatStat(r.views)}</span>
                        </Show>
                        <Show when={formatStat(r.likes)}>
                          <span title="likes">♥ {formatStat(r.likes)}</span>
                        </Show>
                        <Show when={formatStat(r.downloads)}>
                          <span title="downloads">⬇ {formatStat(r.downloads)}</span>
                        </Show>
                      </div>
                    </Show>
                    <Show when={selected()}>
                      <div class="card-selected-badge">✓</div>
                    </Show>
                  </div>
                );
              }}
            </For>
          </div>
        </Show>

        <Show when={!searching() && !searchInfo() && !searchError()}>
          <div class="empty-state">
            Type a query above to search Pixabay, Pexels, and Unsplash. Configure
            API keys in the Settings tab. Toggle Photos/Videos/Both to switch
            media types — Unsplash returns photos only.
          </div>
        </Show>
      </Show>

      <Show when={mode() === "library"}>
        <div class="library-toolbar">
          <input
            type="search"
            placeholder="Filter by query, alt, author or tag..."
            value={filterText()}
            onInput={(e) => setFilterText(e.currentTarget.value)}
            class="library-filter"
          />
          <select
            value={filterSource()}
            onChange={(e) => setFilterSource(e.currentTarget.value)}
          >
            <option value="">All sources</option>
            <For each={sources()}>{(s) => <option value={s}>{s}</option>}</For>
          </select>
          <select
            value={filterKind()}
            onChange={(e) => setFilterKind(e.currentTarget.value)}
          >
            <option value="">All kinds</option>
            <For each={kinds()}>{(k) => <option value={k}>{k}</option>}</For>
          </select>
          <select
            value={sortField()}
            onChange={(e) => setSortField(e.currentTarget.value as any)}
          >
            <option value="downloaded_at">Most recent</option>
            <option value="width">Width</option>
            <option value="source">Source</option>
          </select>
          <button type="button" onClick={() => setSortDesc(!sortDesc())}>
            {sortDesc() ? "↓ Desc" : "↑ Asc"}
          </button>
          <button type="button" onClick={refreshLibrary} disabled={libLoading()}>
            {libLoading() ? "Loading..." : "Refresh"}
          </button>
        </div>

        <Show when={libError()}>
          <div class="error-banner">Error: {libError()}</div>
        </Show>

        <Show when={images().length > 0 || filterText() || filterSource() || filterKind()}>
          <div class="batch-bar">
            <div class="batch-info">
              {librarySelection().size} of {filteredImages().length} selected
              <Show when={filteredImages().length !== images().length}>
                <span class="library-filter-hint">
                  {" "}· {images().length - filteredImages().length} hidden
                </span>
              </Show>
            </div>
            <div class="batch-actions">
              <button type="button" onClick={selectAllLibrary} disabled={busy()}>
                Select all
              </button>
              <button type="button" onClick={clearLibrarySel} disabled={busy()}>
                Clear
              </button>
              <button
                type="button"
                class="danger"
                onClick={deleteSelected}
                disabled={busy() || librarySelection().size === 0}
              >
                Delete selected
              </button>
            </div>
          </div>
        </Show>

        <Show
          when={filteredImages().length > 0}
          fallback={
            <div class="empty-state">
              <Show
                when={images().length === 0}
                fallback={<>No items match the current filter.</>}
              >
                Library is empty. Switch to Search to find and download media.
              </Show>
            </div>
          }
        >
          <div class="grid">
            <For each={filteredImages()}>
              {(img) => {
                const selected = () => librarySelection().has(img.id);
                return (
                  <div
                    class="card"
                    classList={{ selected: selected(), [`kind-${img.kind}`]: true }}
                    onClick={() => toggleLibrarySel(img.id)}
                  >
                    <div class="card-image-wrap">
                      <LibraryThumb id={img.id} alt={img.alt || img.query} />
                      <Show when={img.kind === "video"}>
                        <span class="kind-badge video-badge">▶ {formatDuration(img.duration_secs)}</span>
                      </Show>
                      <Show when={img.kind !== "video" && img.kind !== "photo"}>
                        <span class="kind-badge">{img.kind}</span>
                      </Show>
                    </div>
                    <div class="card-meta">
                      <span class="card-source">{img.source}</span>
                      <span class="card-dims">
                        {img.width}×{img.height}
                      </span>
                    </div>
                    <Show when={img.author_name}>
                      <div class="card-author-line" title={img.author_name}>
                        by {img.author_name}
                      </div>
                    </Show>
                    <Show when={img.alt}>
                      <div class="card-alt" title={img.alt}>
                        {img.alt}
                      </div>
                    </Show>
                    <Show when={selected()}>
                      <div class="card-selected-badge">✓</div>
                    </Show>
                  </div>
                );
              }}
            </For>
          </div>
        </Show>
      </Show>
    </div>
  );
};

const SearchBar: Component<{
  query: string;
  onQueryChange: (v: string) => void;
  pixabayPages: number;
  pexelsPages: number;
  unsplashPages: number;
  onPixabayChange: (v: number) => void;
  onPexelsChange: (v: number) => void;
  onUnsplashChange: (v: number) => void;
  kind: SearchKindParam;
  onKindChange: (v: SearchKindParam) => void;
  searching: boolean;
  onSearch: (e?: Event) => void;
}> = (props) => {
  return (
    <form class="search-bar" onSubmit={props.onSearch}>
      <input
        type="text"
        class="search-input"
        placeholder="Search Pixabay + Pexels + Unsplash..."
        value={props.query}
        onInput={(e) => props.onQueryChange(e.currentTarget.value)}
      />
      <div class="kind-toggle" role="radiogroup">
        <button
          type="button"
          classList={{ active: props.kind === "photo" }}
          onClick={() => props.onKindChange("photo")}
        >
          Photos
        </button>
        <button
          type="button"
          classList={{ active: props.kind === "video" }}
          onClick={() => props.onKindChange("video")}
        >
          Videos
        </button>
        <button
          type="button"
          classList={{ active: props.kind === "both" }}
          onClick={() => props.onKindChange("both")}
        >
          Both
        </button>
      </div>
      <PageSelector
        label="Pixabay"
        value={props.pixabayPages}
        onChange={props.onPixabayChange}
        max={5}
      />
      <PageSelector
        label="Pexels"
        value={props.pexelsPages}
        onChange={props.onPexelsChange}
        max={5}
      />
      <PageSelector
        label="Unsplash"
        value={props.unsplashPages}
        onChange={props.onUnsplashChange}
        max={5}
      />
      <button
        type="submit"
        class="primary"
        disabled={props.searching || !props.query.trim()}
      >
        {props.searching ? "Searching..." : "Search"}
      </button>
    </form>
  );
};

const PageSelector: Component<{
  label: string;
  value: number;
  onChange: (v: number) => void;
  max: number;
}> = (props) => {
  return (
    <label class="page-selector">
      <span class="page-label">{props.label}</span>
      <input
        type="number"
        min="0"
        max={props.max}
        value={props.value}
        onInput={(e) =>
          props.onChange(
            Math.max(0, Math.min(props.max, Number(e.currentTarget.value) || 0))
          )
        }
      />
    </label>
  );
};

export default ImagesTab;
