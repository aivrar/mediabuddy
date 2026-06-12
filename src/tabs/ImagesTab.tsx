import {
  createEffect,
  createMemo,
  createSignal,
  For,
  on,
  onCleanup,
  onMount,
  Show,
  type Component,
} from "solid-js";
import {
  api,
  type Image,
  type QuotaSlot,
  type QuotaSnapshot,
  type SearchFilters,
  type SearchKindParam,
  type SearchResult,
  type Topic,
  type TopicProgress,
  type TopicStatus,
  type TopicSummary,
} from "../lib/api";
import {
  dropThumb,
  getImageUrl,
  getMediaFileUrl,
  getThumbUrl,
} from "../lib/thumbCache";
import LibraryThumb from "../components/LibraryThumb";
import "./ImagesTab.css";

type SourceId = "pixabay" | "pexels" | "unsplash";
const SOURCE_IDS: SourceId[] = ["pixabay", "pexels", "unsplash"];
type VisionCaptionMode = "missing" | "short" | "overwrite" | "skip";
type VisionCaptionTask = "caption" | "detailed" | "more_detailed";

const FILTERS_KEY = "mediabuddy.search.filters.v1";
const SOURCES_KEY = "mediabuddy.search.sources.v1";
const COUNT_KEY = "mediabuddy.search.count.v1";
const DOWNLOAD_CONCURRENCY_KEY = "mediabuddy.download.concurrency.v1";
const VISION_CAPTION_MODE_KEY = "mediabuddy.vision.captionMode.v1";
const VISION_CAPTION_TASK_KEY = "mediabuddy.vision.captionTask.v1";
const VISION_CAPTION_MIN_CHARS_KEY = "mediabuddy.vision.captionMinChars.v1";
const VISION_DETECT_OBJECTS_KEY = "mediabuddy.vision.detectObjects.v1";
const DEFAULT_VISION_CAPTION_MIN_CHARS = 80;

/**
 * Only allow http(s) URLs as link targets. Provider-supplied author /
 * source-page URLs are untrusted; a `javascript:` (or `data:`) href would
 * execute in the privileged webview when clicked. Returns undefined for
 * anything that isn't http/https, so the anchor renders without a target.
 */
function safeHref(url: string | undefined | null): string | undefined {
  if (!url) return undefined;
  const u = url.trim();
  if (!u) return undefined;
  try {
    const parsed = new URL(u);
    return parsed.protocol === "http:" || parsed.protocol === "https:"
      ? parsed.href
      : undefined;
  } catch {
    return undefined;
  }
}

type LibrarySortField = "downloaded_at" | "width" | "source";
const LIBRARY_SORT_FIELDS: LibrarySortField[] = ["downloaded_at", "width", "source"];

function isLibrarySortField(value: string): value is LibrarySortField {
  return (LIBRARY_SORT_FIELDS as string[]).includes(value);
}

const DEFAULT_FILTERS: SearchFilters = {
  orientation: "any",
  color: undefined,
  min_width: undefined,
  min_height: undefined,
  category: undefined,
  order: "popular",
  size: undefined,
  safesearch: true,
  editors_choice: false,
  exclude_ai: false,
};

const COLOR_SWATCHES: { id: string; label: string; hex: string }[] = [
  { id: "red", label: "Red", hex: "#e53935" },
  { id: "orange", label: "Orange", hex: "#fb8c00" },
  { id: "yellow", label: "Yellow", hex: "#fdd835" },
  { id: "green", label: "Green", hex: "#43a047" },
  { id: "turquoise", label: "Turquoise", hex: "#26a69a" },
  { id: "blue", label: "Blue", hex: "#1e88e5" },
  { id: "purple", label: "Purple", hex: "#8e24aa" },
  { id: "pink", label: "Pink", hex: "#ec407a" },
  { id: "brown", label: "Brown", hex: "#795548" },
  { id: "white", label: "White", hex: "#fafafa" },
  { id: "gray", label: "Gray", hex: "#757575" },
  { id: "black", label: "Black", hex: "#212121" },
];

// Per-provider page caps. Used both in the budget preview and to mirror the
// backend's `paginate()`. Keep in sync with src-tauri/src/search.rs.
const SOURCE_CAPS: Record<SourceId, number> = {
  pixabay: 200,
  pexels: 80,
  unsplash: 30,
};
const UNSPLASH_DETAIL_THRESHOLD = 30;

type BudgetBreakdown = {
  total: number;
  perSource: { id: SourceId; calls: number; note?: string }[];
};

const PIXABAY_CATEGORIES = [
  "backgrounds",
  "fashion",
  "nature",
  "science",
  "education",
  "feelings",
  "health",
  "people",
  "religion",
  "places",
  "animals",
  "industry",
  "computer",
  "food",
  "sports",
  "transportation",
  "travel",
  "buildings",
  "business",
  "music",
];

type Mode = "search" | "library";

function computeBudget(
  count: number,
  enabled: Set<SourceId>,
  kind: SearchKindParam,
  unsplashDetailThreshold: number = UNSPLASH_DETAIL_THRESHOLD
): BudgetBreakdown {
  const perSource: BudgetBreakdown["perSource"] = [];
  let total = 0;
  for (const id of SOURCE_IDS) {
    if (!enabled.has(id)) continue;
    const cap = SOURCE_CAPS[id];
    const pages = Math.max(1, Math.ceil(count / cap));
    let calls = 0;
    let note: string | undefined;
    if (id === "pixabay") {
      if (kind === "photo") calls = pages;
      else if (kind === "video") calls = pages;
      else calls = pages * 2;
    } else if (id === "pexels") {
      if (kind === "photo") calls = pages;
      else if (kind === "video") calls = pages;
      else calls = pages * 2;
    } else {
      // unsplash — photos only
      if (kind === "video") {
        calls = 0;
        note = "no video API";
      } else {
        calls = pages;
        if (count <= unsplashDetailThreshold) {
          // 1 search + N detail fetches
          calls += count;
          note = `+${count} detail`;
        } else {
          note = "details skipped";
        }
      }
    }
    if (calls > 0) {
      perSource.push({ id, calls, note });
      total += calls;
    } else if (note) {
      perSource.push({ id, calls: 0, note });
    }
  }
  return { total, perSource };
}

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

const formatBytes = (n: number | null | undefined) => {
  if (n == null) return "—";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
};

const resultPreviewUrl = (r: SearchResult) =>
  r.urls?.poster ||
  r.urls?.webformat ||
  r.urls?.medium ||
  r.urls?.regular ||
  r.urls?.small ||
  r.url ||
  "";

const resultFullUrl = (r: SearchResult) =>
  r.urls?.raw ||
  r.urls?.full ||
  r.urls?.large ||
  r.urls?.original ||
  r.urls?.regular ||
  resultPreviewUrl(r);

const resultPosterUrl = (r: SearchResult) => r.urls?.poster || resultPreviewUrl(r);

const resultVideoPreviewUrl = (r: SearchResult) => {
  if (r.kind.toLowerCase() !== "video") return "";
  const urls = r.urls ?? {};
  const preferred = [
    urls.preview,
    urls.tiny,
    urls.small,
    urls.medium,
    urls["sd_video/mp4"],
    urls["video/mp4"],
    urls["hd_video/mp4"],
    urls.large,
    r.url,
  ];
  const direct = preferred.find((url) => typeof url === "string" && url.length > 0);
  if (direct) return direct;

  const candidates = Object.entries(urls)
    .filter(([key, url]) => {
      const k = key.toLowerCase();
      return (
        typeof url === "string" &&
        url.length > 0 &&
        (k.includes("mp4") || k.includes("video")) &&
        !k.includes("hls")
      );
    })
    .sort(([a], [b]) => videoPreviewRank(a) - videoPreviewRank(b));
  return candidates[0]?.[1] ?? "";
};

const videoPreviewRank = (key: string) => {
  const k = key.toLowerCase();
  if (k.includes("tiny") || k.includes("small") || k.includes("sd")) return 0;
  if (k.includes("medium")) return 1;
  if (k.includes("large") || k.includes("hd")) return 2;
  return 3;
};

const clampDownloadConcurrency = (n: number) =>
  Math.max(1, Math.min(32, Math.round(n)));
const clampVisionCaptionMinChars = (n: number) =>
  Math.max(1, Math.min(1000, Math.round(n)));

const ImagesTab: Component = () => {
  const [mode, setMode] = createSignal<Mode>("library");

  // ---- Library state ----
  const [images, setImages] = createSignal<Image[]>([]);
  const [libLoading, setLibLoading] = createSignal(false);
  const [libError, setLibError] = createSignal<string | null>(null);
  const [libInfo, setLibInfo] = createSignal<string | null>(null);
  const [librarySelection, setLibrarySelection] = createSignal(new Set<string>());
  const [lastSelectedId, setLastSelectedId] = createSignal<string | null>(null);
  const [filterText, setFilterText] = createSignal("");
  const [facetSources, setFacetSources] = createSignal<Set<string>>(new Set());
  const [facetKinds, setFacetKinds] = createSignal<Set<string>>(new Set());
  const [facetCaption, setFacetCaption] = createSignal<"any" | "captioned" | "uncaptioned">(
    "any"
  );
  const [facetTopicId, setFacetTopicId] = createSignal<string | null>(null);
  const [facetTopicIds, setFacetTopicIds] = createSignal<Set<string> | null>(null);

  const setLibraryTopicFilter = async (id: string | null) => {
    setFacetTopicId(id);
    if (id == null) {
      setFacetTopicIds(null);
      return;
    }
    try {
      const ids = await api.topicImageIds(id);
      setFacetTopicIds(new Set<string>(ids));
    } catch {
      setFacetTopicIds(new Set<string>());
    }
  };
  const [sortField, setSortField] = createSignal<LibrarySortField>("downloaded_at");
  const [sortDesc, setSortDesc] = createSignal(true);
  const [busy, setBusy] = createSignal(false);
  const [visionBusy, setVisionBusy] = createSignal(false);
  const [visionCaptionMode, setVisionCaptionModeState] =
    createSignal<VisionCaptionMode>(loadVisionCaptionMode());
  const [visionCaptionTask, setVisionCaptionTaskState] =
    createSignal<VisionCaptionTask>(loadVisionCaptionTask());
  const [visionCaptionMinChars, setVisionCaptionMinCharsState] =
    createSignal(loadVisionCaptionMinChars());
  const [visionDetectObjects, setVisionDetectObjectsState] =
    createSignal(loadVisionDetectObjects());
  const [inspectorId, setInspectorId] = createSignal<string | null>(null);

  const setVisionCaptionMode = (mode: VisionCaptionMode) => {
    setVisionCaptionModeState(mode);
    saveVisionCaptionMode(mode);
  };
  const setVisionCaptionTask = (task: VisionCaptionTask) => {
    setVisionCaptionTaskState(task);
    saveVisionCaptionTask(task);
  };
  const setVisionCaptionMinChars = (value: number) => {
    const clamped = clampVisionCaptionMinChars(value);
    setVisionCaptionMinCharsState(clamped);
    saveVisionCaptionMinChars(clamped);
  };
  const setVisionDetectObjects = (value: boolean) => {
    setVisionDetectObjectsState(value);
    saveVisionDetectObjects(value);
  };

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

  // Sort separately from filtering so that typing in the filter box (which
  // changes filterText) doesn't re-sort the entire library on every keystroke
  // — it only re-runs the cheap filter pass over the already-sorted list.
  const sortedImages = createMemo(() => {
    const f = sortField();
    const desc = sortDesc();
    return [...images()].sort((a, b) => {
      let cmp: number;
      if (f === "width") cmp = a.width - b.width;
      else if (f === "source") cmp = a.source.localeCompare(b.source);
      else cmp = a.downloaded_at.localeCompare(b.downloaded_at);
      return desc ? -cmp : cmp;
    });
  });

  const filteredImages = createMemo(() => {
    let list = sortedImages();
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
    const fs = facetSources();
    if (fs.size > 0) list = list.filter((i) => fs.has(i.source));
    const fk = facetKinds();
    if (fk.size > 0) list = list.filter((i) => fk.has(i.kind));
    const fc = facetCaption();
    if (fc === "captioned") list = list.filter((i) => i.alt && i.alt.length > 0);
    else if (fc === "uncaptioned") list = list.filter((i) => !i.alt);
    const ft = facetTopicIds();
    if (ft) list = list.filter((i) => ft.has(i.id));
    return list;
  });

  // Cap how many cards render at once; "Load more" reveals further pages.
  // Bounds DOM size and the per-card IntersectionObserver count for large
  // libraries. Inspector navigation and select-all still use the full set.
  const LIBRARY_PAGE = 300;
  const [libVisible, setLibVisible] = createSignal(LIBRARY_PAGE);
  const displayedImages = createMemo(() => filteredImages().slice(0, libVisible()));
  // Reset to the first page whenever the filtered/sorted set changes.
  createEffect(
    on(
      [
        filterText,
        facetSources,
        facetKinds,
        facetCaption,
        facetTopicIds,
        sortField,
        sortDesc,
      ],
      () => {
        setLibVisible(LIBRARY_PAGE);
        // Drop selected ids that are no longer visible under the new filter,
        // so bulk actions can't operate on now-hidden items.
        const visible = new Set(filteredImages().map((i) => i.id));
        setLibrarySelection((sel) => {
          const next = new Set<string>();
          for (const id of sel) if (visible.has(id)) next.add(id);
          return next;
        });
      },
      { defer: true }
    )
  );

  const facetSourceCounts = createMemo(() => {
    const counts = new Map<string, number>();
    for (const i of images()) counts.set(i.source, (counts.get(i.source) ?? 0) + 1);
    return Array.from(counts.entries()).sort((a, b) => b[1] - a[1]);
  });
  const facetKindCounts = createMemo(() => {
    const counts = new Map<string, number>();
    for (const i of images()) counts.set(i.kind, (counts.get(i.kind) ?? 0) + 1);
    return Array.from(counts.entries()).sort((a, b) => b[1] - a[1]);
  });
  const captionCounts = createMemo(() => {
    let cap = 0;
    let uncap = 0;
    for (const i of images()) {
      if (i.alt && i.alt.length > 0) cap++;
      else uncap++;
    }
    return { captioned: cap, uncaptioned: uncap };
  });

  const toggleFacet = (
    set: Set<string>,
    setter: (s: Set<string>) => void,
    val: string
  ) => {
    const next = new Set(set);
    if (next.has(val)) next.delete(val);
    else next.add(val);
    setter(next);
  };

  const handleCardClick = (id: string, ev: MouseEvent) => {
    // Shift-click: range select between lastSelectedId and id
    if (ev.shiftKey && lastSelectedId()) {
      const list = filteredImages();
      const a = list.findIndex((i) => i.id === lastSelectedId());
      const b = list.findIndex((i) => i.id === id);
      if (a >= 0 && b >= 0) {
        const [lo, hi] = a < b ? [a, b] : [b, a];
        const next = new Set(librarySelection());
        for (let i = lo; i <= hi; i++) next.add(list[i].id);
        setLibrarySelection(next);
        setLastSelectedId(id);
        return;
      }
    }
    // Cmd/Ctrl-click or plain click: toggle
    const sel = new Set(librarySelection());
    if (sel.has(id)) sel.delete(id);
    else sel.add(id);
    setLibrarySelection(sel);
    setLastSelectedId(id);
  };

  const selectAllLibrary = () => {
    setLibrarySelection(new Set(filteredImages().map((i) => i.id)));
  };
  const clearLibrarySel = () => {
    setLibrarySelection(new Set<string>());
    setLastSelectedId(null);
  };

  const inspectorImage = createMemo(() => {
    const id = inspectorId();
    if (!id) return null;
    return images().find((i) => i.id === id) || null;
  });

  const inspectorIndex = createMemo(() => {
    const id = inspectorId();
    if (!id) return -1;
    return filteredImages().findIndex((i) => i.id === id);
  });
  const inspectorTotal = () => filteredImages().length;

  const navInspector = (delta: number) => {
    const list = filteredImages();
    if (list.length === 0) return;
    const cur = inspectorIndex();
    if (cur < 0) {
      setInspectorId(list[0].id);
      return;
    }
    const next = (cur + delta + list.length) % list.length;
    setInspectorId(list[next].id);
  };

  const updateImageInPlace = (id: string, patch: Partial<Image>) => {
    setImages(images().map((i) => (i.id === id ? { ...i, ...patch } : i)));
  };

  const runVisionForIds = async (ids: string[]) => {
    const unique = Array.from(new Set(ids)).filter(Boolean);
    if (unique.length === 0 || busy()) return;
    const byId = new Map(images().map((img) => [img.id, img]));
    const eligible = unique.filter((id) => {
      const image = byId.get(id);
      return (
        image &&
        image.kind.toLowerCase() !== "video" &&
        !image.preview_only &&
        !!image.path
      );
    });
    const locallySkipped = unique.length - eligible.length;
    if (eligible.length === 0) {
      setLibError(null);
      setLibInfo("Florence-2 analyzes downloaded still images only.");
      return;
    }
    setBusy(true);
    setVisionBusy(true);
    setLibError(null);
    setLibInfo(
      `Florence-2 analyzing ${eligible.length} item${eligible.length === 1 ? "" : "s"}${locallySkipped > 0 ? ` (${locallySkipped} skipped)` : ""}...`
    );
    try {
      const summary = await api.visionAnalyzeImages(eligible, {
        detectObjects: visionDetectObjects(),
        captionMode: visionCaptionMode(),
        captionTask: visionCaptionTask(),
        captionMinChars: visionCaptionMinChars(),
        loadIfNeeded: true,
      });
      const updated = new Map(
        summary.results
          .filter((r) => r.image)
          .map((r) => [r.image!.id, r.image!])
      );
      if (updated.size > 0) {
        setImages(images().map((img) => updated.get(img.id) ?? img));
      }
      const addedTags = summary.results.reduce(
        (sum, r) => sum + r.tags_added.length,
        0
      );
      const writtenCaptions = summary.results.filter((r) => r.caption_written).length;
      const skipped = summary.skipped + locallySkipped;
      setLibInfo(
        `Florence-2 done: ${summary.analyzed} analyzed, ${skipped} skipped, ${summary.failed} failed, ${writtenCaptions} caption${writtenCaptions === 1 ? "" : "s"} written, ${addedTags} tag${addedTags === 1 ? "" : "s"} added.`
      );
      if (summary.failed > 0) {
        const first = summary.results.find((r) => !r.ok && !r.skipped && r.error);
        if (first?.error) setLibError(`Florence-2: ${first.error}`);
      }
    } catch (e) {
      setLibError(`Florence-2: ${String(e)}`);
      setLibInfo(null);
    } finally {
      setBusy(false);
      setVisionBusy(false);
    }
  };

  const bulkAppendTags = async (raw: string) => {
    const newTags = raw
      .split(",")
      .map((t) => t.trim())
      .filter((t) => t.length > 0);
    if (newTags.length === 0) return;
    const ids = Array.from(librarySelection());
    if (ids.length === 0) return;
    setBusy(true);
    try {
      const idSet = new Set(ids);
      // Compute merged tags per selected image up front, persist each, then
      // apply ALL changes in a single setImages — avoids rebuilding the whole
      // images array (and re-running the filter/sort memos) once per item.
      const merges = new Map<string, string[]>();
      for (const img of images()) {
        if (!idSet.has(img.id)) continue;
        merges.set(img.id, Array.from(new Set([...img.tags, ...newTags])).sort());
      }
      for (const [id, merged] of merges) {
        await api.updateImage(id, { tags: merged });
      }
      setImages(
        images().map((i) =>
          merges.has(i.id) ? { ...i, tags: merges.get(i.id)! } : i
        )
      );
    } catch (e) {
      setLibError(String(e));
    } finally {
      setBusy(false);
    }
  };

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

  // ---- Quota state (polled) ----
  const [quota, setQuota] = createSignal<QuotaSnapshot | null>(null);
  const [detailThreshold, setDetailThreshold] = createSignal<number>(
    UNSPLASH_DETAIL_THRESHOLD
  );
  let quotaTimer: number | undefined;
  const refreshQuota = async () => {
    try {
      setQuota(await api.getQuotaStatus());
    } catch {
      // ignore — backend not ready yet
    }
  };
  const refreshSettings = async () => {
    try {
      const s = await api.getSettings();
      if (typeof s.unsplash_detail_threshold === "number") {
        setDetailThreshold(s.unsplash_detail_threshold);
      }
    } catch {
      // ignore
    }
  };
  onMount(() => {
    refreshQuota();
    refreshSettings();
    refreshTopicList();
    quotaTimer = window.setInterval(refreshQuota, 4000);
    window.addEventListener("keydown", onGlobalKey);
  });
  onCleanup(() => {
    if (quotaTimer !== undefined) window.clearInterval(quotaTimer);
    window.removeEventListener("keydown", onGlobalKey);
  });

  const onGlobalKey = (ev: KeyboardEvent) => {
    if (inspectorId() == null) return;
    // Don't intercept arrow keys when the user is editing text in the
    // inspector textareas / inputs.
    const t = ev.target as HTMLElement | null;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) return;
    if (ev.key === "ArrowRight" || ev.key === "j") {
      ev.preventDefault();
      navInspector(1);
    } else if (ev.key === "ArrowLeft" || ev.key === "k") {
      ev.preventDefault();
      navInspector(-1);
    } else if (ev.key === "Escape") {
      setInspectorId(null);
    }
  };

  // ---- Search state ----
  const [query, setQuery] = createSignal("");
  const [enabledSources, setEnabledSources] = createSignal<Set<SourceId>>(
    loadSourceSet()
  );
  const [resultsPerSource, setResultsPerSource] = createSignal<number>(loadCount());
  const [filters, setFilters] = createSignal<SearchFilters>(loadFilters());
  const [filtersOpen, setFiltersOpen] = createSignal(false);
  const [searchKind, setSearchKind] = createSignal<SearchKindParam>("photo");
  const [results, setResults] = createSignal<SearchResult[]>([]);
  const [resultsSelection, setResultsSelection] = createSignal(new Set<string>());
  const [searchInspectorUrl, setSearchInspectorUrl] = createSignal<string | null>(null);
  const [downloadingUrls, setDownloadingUrls] = createSignal(new Set<string>());
  const [downloadConcurrency, setDownloadConcurrency] = createSignal(
    loadDownloadConcurrency()
  );
  const [searching, setSearching] = createSignal(false);
  const [searchInfo, setSearchInfo] = createSignal<string | null>(null);
  const [searchError, setSearchError] = createSignal<string | null>(null);
  const [downloadInfo, setDownloadInfo] = createSignal<string | null>(null);
  const [currentTopic, setCurrentTopic] = createSignal<Topic | null>(null);
  const [topicStatus, setTopicStatus] = createSignal<TopicStatus | null>(null);
  const [lastProgress, setLastProgress] = createSignal<TopicProgress[] | null>(null);
  const [topicList, setTopicList] = createSignal<TopicSummary[]>([]);
  const [topicsSidebarOpen, setTopicsSidebarOpen] = createSignal(true);

  const refreshTopicStatus = async (id?: string) => {
    const tid = id ?? currentTopic()?.id;
    if (!tid) return;
    try {
      const st = await api.topicStatus(tid);
      setTopicStatus(st);
    } catch {}
  };

  const refreshTopicList = async () => {
    try {
      setTopicList(await api.topicList());
    } catch {}
  };

  const updateFilter = <K extends keyof SearchFilters>(
    key: K,
    value: SearchFilters[K]
  ) => {
    const next = { ...filters(), [key]: value };
    setFilters(next);
    saveFilters(next);
  };

  const clearFilters = () => {
    setFilters({ ...DEFAULT_FILTERS });
    saveFilters({ ...DEFAULT_FILTERS });
  };

  const toggleSource = (s: SourceId) => {
    const set = new Set(enabledSources());
    if (set.has(s)) set.delete(s);
    else set.add(s);
    setEnabledSources(set);
    saveSourceSet(set);
  };

  const setCount = (n: number) => {
    const v = Math.max(1, Math.min(500, Math.round(n)));
    setResultsPerSource(v);
    saveCount(v);
  };

  const budget = createMemo(() =>
    computeBudget(
      resultsPerSource(),
      enabledSources(),
      searchKind(),
      detailThreshold()
    )
  );

  const activeFilterCount = createMemo(() => {
    const f = filters();
    let n = 0;
    if (f.orientation && f.orientation !== "any") n++;
    if (f.color) n++;
    if (f.min_width) n++;
    if (f.min_height) n++;
    if (f.category) n++;
    if (f.order && f.order !== "popular") n++;
    if (f.size) n++;
    if (f.safesearch === false) n++;
    if (f.editors_choice) n++;
    if (f.exclude_ai) n++;
    return n;
  });

  const searchInspectorResult = createMemo(() => {
    const url = searchInspectorUrl();
    if (!url) return null;
    return results().find((r) => r.url === url) || null;
  });

  const searchInspectorIndex = createMemo(() => {
    const url = searchInspectorUrl();
    if (!url) return -1;
    return results().findIndex((r) => r.url === url);
  });

  const navSearchInspector = (delta: number) => {
    const list = results();
    if (list.length === 0) return;
    const cur = searchInspectorIndex();
    if (cur < 0) {
      setSearchInspectorUrl(list[0].url);
      return;
    }
    const next = (cur + delta + list.length) % list.length;
    setSearchInspectorUrl(list[next].url);
  };

  const nextInspectorUrlAfterRemoving = (
    removed: Set<string>,
    currentUrl = searchInspectorUrl()
  ) => {
    const list = results();
    if (list.length === 0) return null;
    if (currentUrl && !removed.has(currentUrl)) return currentUrl;

    const cur = currentUrl ? list.findIndex((r) => r.url === currentUrl) : -1;
    const start = cur >= 0 ? cur : 0;
    for (let offset = 0; offset < list.length; offset++) {
      const candidate = list[(start + offset) % list.length];
      if (!removed.has(candidate.url)) return candidate.url;
    }
    return null;
  };

  const setResultDownloading = (url: string, downloading: boolean) => {
    setDownloadingUrls((prev) => {
      const next = new Set(prev);
      if (downloading) next.add(url);
      else next.delete(url);
      return next;
    });
  };

  const setBatchConcurrency = (n: number) => {
    const value = clampDownloadConcurrency(n);
    setDownloadConcurrency(value);
    saveDownloadConcurrency(value);
  };

  createEffect(
    on(
      results,
      (list) => {
        const url = searchInspectorUrl();
        if (list.length === 0) {
          setSearchInspectorUrl(null);
          return;
        }
        if (url && !list.some((r) => r.url === url)) {
          setSearchInspectorUrl(list[0].url);
        }
      },
      { defer: true }
    )
  );

  // Run a topic round: find_or_create matching the current bar state, then
  // get_more. If the topic already exists, this resumes pagination.
  const onSearch = async (e?: Event) => {
    e?.preventDefault();
    const q = query().trim();
    if (!q) return;
    const enabled = enabledSources();
    if (enabled.size === 0) {
      setSearchError("Pick at least one source.");
      return;
    }

    setSearching(true);
    setSearchError(null);
    setSearchInfo(null);
    setDownloadInfo(null);
    setResults([]);
    setResultsSelection(new Set<string>());
    setSearchInspectorUrl(null);
    setLastProgress(null);

    try {
      const topic = await api.topicFindOrCreate({
        query: q,
        filters: filters(),
        kind: searchKind(),
        sources: Array.from(enabled),
      });
      setCurrentTopic(topic);

      const t0 = performance.now();
      const round = await api.topicGetMore(topic.id, resultsPerSource());
      const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
      setResults(round.results);
      setSearchInspectorUrl(round.results[0]?.url ?? null);
      setLastProgress(round.progress);

      const ph = round.results.filter((x) => x.kind === "photo").length;
      const vd = round.results.filter((x) => x.kind === "video").length;
      const breakdown =
        searchKind() === "both"
          ? ` (${ph} photo${ph === 1 ? "" : "s"} + ${vd} video${vd === 1 ? "" : "s"})`
          : "";
      setSearchInfo(
        `${round.results.length} new${breakdown} for "${q}" in ${elapsed}s`
      );
      await refreshTopicStatus(topic.id);
      await refreshTopicList();
    } catch (err) {
      setSearchError(String(err));
    } finally {
      setSearching(false);
      refreshQuota();
      refreshSettings();
    }
  };

  // Continue the current topic without re-finding. Each press fetches the
  // next page from every still-active cursor.
  const onGetMore = async () => {
    const topic = currentTopic();
    if (!topic) return;
    setSearching(true);
    setSearchError(null);
    setLastProgress(null);
    try {
      const t0 = performance.now();
      const round = await api.topicGetMore(topic.id, resultsPerSource());
      const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
      // Merge: append, don't replace.
      const merged = [...results(), ...round.results];
      setResults(merged);
      if (!searchInspectorUrl() && merged.length > 0) {
        setSearchInspectorUrl(merged[0].url);
      }
      setLastProgress(round.progress);
      const newCount = round.results.length;
      const someExhausted = round.progress.some((p) => p.status === "empty");
      setSearchInfo(
        `+${newCount} new in ${elapsed}s${someExhausted ? " · some sources exhausted" : ""}`
      );
      await refreshTopicStatus(topic.id);
      await refreshTopicList();
    } catch (err) {
      setSearchError(String(err));
    } finally {
      setSearching(false);
      refreshQuota();
    }
  };

  const onResetTopic = async () => {
    const topic = currentTopic();
    if (!topic) return;
    if (
      !confirm(
        `Reset "${topic.query}"? This wipes pagination cursors and the seen list — the next round will start at page 1 and may re-show results you've already evaluated.`
      )
    )
      return;
    try {
      await api.topicReset(topic.id);
      setResults([]);
      setSearchInspectorUrl(null);
      setLastProgress(null);
      setSearchInfo("Topic reset — next search starts fresh.");
      await refreshTopicStatus(topic.id);
      await refreshTopicList();
    } catch (err) {
      setSearchError(String(err));
    }
  };

  const onClearTopic = () => {
    setCurrentTopic(null);
    setTopicStatus(null);
    setResults([]);
    setSearchInspectorUrl(null);
    setLastProgress(null);
    setSearchInfo(null);
  };

  // Fill mode: loop topic_get_more rounds until N new uniques are gathered,
  // every cursor reports 'empty', or the user hits Stop. Each round's worth
  // of progress is announced live; the user can stop at any time.
  const [fillTarget, setFillTarget] = createSignal(100);
  const [fillRunning, setFillRunning] = createSignal(false);
  const [fillStopRequested, setFillStopRequested] = createSignal(false);
  const [fillProgress, setFillProgress] = createSignal<{
    gathered: number;
    rounds: number;
    apiCalls: number;
    exhaustedSources: string[];
  }>({ gathered: 0, rounds: 0, apiCalls: 0, exhaustedSources: [] });

  const onFillN = async () => {
    const topic = currentTopic();
    if (!topic) return;
    const target = fillTarget();
    if (target <= 0) return;
    setFillRunning(true);
    setFillStopRequested(false);
    setSearchError(null);
    setLastProgress(null);
    setFillProgress({ gathered: 0, rounds: 0, apiCalls: 0, exhaustedSources: [] });

    const exhausted = new Set<string>();
    let gathered = 0;
    let rounds = 0;
    let apiCalls = 0;
    const accumulated: SearchResult[] = [];

    try {
      while (gathered < target) {
        if (fillStopRequested()) break;
        rounds++;
        const round = await api.topicGetMore(topic.id, resultsPerSource());
        // Track per-cursor exhaustion + count this round's API calls.
        let stillActive = 0;
        for (const p of round.progress) {
          apiCalls++;
          if (p.status === "empty") {
            exhausted.add(`${p.source}/${p.media_kind}`);
          } else if (p.status !== "error") {
            stillActive++;
          }
        }
        accumulated.push(...round.results);
        gathered += round.results.length;
        setFillProgress({
          gathered,
          rounds,
          apiCalls,
          exhaustedSources: Array.from(exhausted),
        });
        setLastProgress(round.progress);
        // If every cursor is now empty, stop — there's nothing left.
        if (stillActive === 0) break;
        // Safety: cap at 50 rounds so a misbehaving topic can't loop forever.
        if (rounds >= 50) break;
        // Tiny pause between rounds — keeps UI responsive and avoids
        // hammering providers in a tight loop.
        await new Promise((r) => setTimeout(r, 75));
      }
      setResults([...results(), ...accumulated]);
      if (!searchInspectorUrl() && accumulated.length > 0) {
        setSearchInspectorUrl(accumulated[0].url);
      }
      const stoppedReason = fillStopRequested()
        ? "stopped"
        : gathered >= target
          ? "target reached"
          : "all sources exhausted";
      setSearchInfo(
        `Fill: +${gathered} new in ${rounds} round${rounds === 1 ? "" : "s"} · ${apiCalls} API call${apiCalls === 1 ? "" : "s"} · ${stoppedReason}`
      );
      await refreshTopicStatus(topic.id);
      await refreshTopicList();
    } catch (err) {
      setSearchError(String(err));
    } finally {
      setFillRunning(false);
      setFillStopRequested(false);
      refreshQuota();
    }
  };

  const onFillStop = () => {
    setFillStopRequested(true);
  };

  // Switch the search shell to a topic from the sidebar — pulls its full
  // record so the bar reflects its filters/kind/sources, then loads cursor
  // status. Doesn't auto-fetch a new round; user clicks "Get more" if they
  // want one.
  const onSelectTopic = async (id: string) => {
    try {
      const st = await api.topicStatus(id);
      if (!st) return;
      setCurrentTopic(st.topic);
      setTopicStatus(st);
      setQuery(st.topic.query);
      setFilters({ ...DEFAULT_FILTERS, ...st.topic.filters });
      saveFilters({ ...DEFAULT_FILTERS, ...st.topic.filters });
      const enabled = new Set<SourceId>(
        (st.topic.enabled_sources as SourceId[]).filter((s) =>
          SOURCE_IDS.includes(s)
        )
      );
      if (enabled.size > 0) {
        setEnabledSources(enabled);
        saveSourceSet(enabled);
      }
      setSearchKind((st.topic.kind as SearchKindParam) || "photo");
      setResults([]);
      setSearchInspectorUrl(null);
      setLastProgress(null);
      setSearchInfo(null);
      setSearchError(null);
    } catch (err) {
      setSearchError(String(err));
    }
  };

  const onRenameTopic = async (id: string, name: string | null) => {
    try {
      await api.topicRename(id, name);
      await refreshTopicList();
      if (currentTopic()?.id === id) await refreshTopicStatus(id);
    } catch (err) {
      setSearchError(String(err));
    }
  };

  const onDeleteTopic = async (id: string) => {
    const t = topicList().find((x) => x.id === id);
    const label = t?.name || t?.query || "this topic";
    if (
      !confirm(
        `Delete topic "${label}"? Its pagination cursors and seen list will be removed. Items already saved to your library are kept.`
      )
    )
      return;
    try {
      await api.topicDelete(id);
      if (currentTopic()?.id === id) onClearTopic();
      // Drop the Library topic-facet filter if it was scoping to this
      // topic — otherwise the grid stays filtered against a deleted id.
      if (facetTopicId() === id) {
        setFacetTopicId(null);
        setFacetTopicIds(null);
      }
      await refreshTopicList();
    } catch (err) {
      setSearchError(String(err));
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
  const clearResultsSel = () => setResultsSelection(new Set<string>());

  const downloadResult = async (item: SearchResult) => {
    if (downloadingUrls().has(item.url)) return;
    setResultDownloading(item.url, true);
    setDownloadInfo("Downloading 1...");
    try {
      const saved = await api.downloadImages([item], { concurrency: 1 });
      setDownloadInfo(
        saved.length > 0
          ? "Saved 1 item. Switch to Library to see it."
          : "No item was saved."
      );
      if (saved.length > 0) {
        const savedUrls = new Set(saved.map((s) => s.url));
        setSearchInspectorUrl(nextInspectorUrlAfterRemoving(savedUrls, item.url));
        setResults(results().filter((r) => r.url !== item.url));
        const nextSel = new Set(resultsSelection());
        nextSel.delete(item.url);
        setResultsSelection(nextSel);
        await refreshLibrary();
      }
    } catch (e) {
      setDownloadInfo(`Error: ${String(e)}`);
    } finally {
      setResultDownloading(item.url, false);
    }
  };

  const downloadSelected = async () => {
    const sel = resultsSelection();
    const items = results().filter((r) => sel.has(r.url));
    if (!items.length) return;
    setBusy(true);
    setDownloadInfo(`Downloading ${items.length}...`);
    try {
      const t0 = performance.now();
      const saved = await api.downloadImages(items, {
        concurrency: downloadConcurrency(),
      });
      const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
      setDownloadInfo(
        `Saved ${saved.length} of ${items.length} in ${elapsed}s. Switch to Library to see them.`
      );
      const savedUrls = new Set(saved.map((s) => s.url));
      setSearchInspectorUrl(nextInspectorUrlAfterRemoving(savedUrls));
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
      const saved = await api.downloadImages(items, {
        concurrency: downloadConcurrency(),
      });
      const elapsed = ((performance.now() - t0) / 1000).toFixed(2);
      setDownloadInfo(`Saved ${saved.length} of ${items.length} in ${elapsed}s.`);
      const savedUrls = new Set(saved.map((s) => s.url));
      setSearchInspectorUrl(nextInspectorUrlAfterRemoving(savedUrls));
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
        <div
          class="search-layout"
          classList={{ "sidebar-collapsed": !topicsSidebarOpen() }}
        >
          <Show
            when={topicsSidebarOpen()}
            fallback={
              <button
                type="button"
                class="topics-sidebar-tab"
                onClick={() => setTopicsSidebarOpen(true)}
                title="Show topics"
              >
                ▸
              </button>
            }
          >
            <TopicsSidebar
              topics={topicList()}
              currentId={currentTopic()?.id ?? null}
              onSelect={onSelectTopic}
              onRename={onRenameTopic}
              onDelete={onDeleteTopic}
              onNew={onClearTopic}
              onClose={() => setTopicsSidebarOpen(false)}
            />
          </Show>
          <div class="search-main">
        <SearchBar
          query={query()}
          onQueryChange={setQuery}
          enabledSources={enabledSources()}
          onToggleSource={toggleSource}
          count={resultsPerSource()}
          onCountChange={setCount}
          kind={searchKind()}
          onKindChange={setSearchKind}
          searching={searching()}
          onSearch={onSearch}
          filtersOpen={filtersOpen()}
          onToggleFilters={() => setFiltersOpen(!filtersOpen())}
          activeFilterCount={activeFilterCount()}
          budget={budget()}
          quota={quota()}
        />

        <Show when={filtersOpen()}>
          <FiltersPanel
            filters={filters()}
            onChange={updateFilter}
            onClear={clearFilters}
            kind={searchKind()}
          />
        </Show>

        <Show when={currentTopic()}>
          <TopicBar
            topic={currentTopic()!}
            status={topicStatus()}
            lastProgress={lastProgress()}
            onGetMore={onGetMore}
            onReset={onResetTopic}
            onClear={onClearTopic}
            busy={searching() || fillRunning()}
            fillTarget={fillTarget()}
            onFillTargetChange={setFillTarget}
            fillRunning={fillRunning()}
            fillProgress={fillProgress()}
            onFillN={onFillN}
            onFillStop={onFillStop}
          />
        </Show>

        <Show when={searchInfo()}>
          <div class="info-banner">{searchInfo()}</div>
        </Show>
        <Show when={downloadInfo()}>
          <div class="info-banner">{downloadInfo()}</div>
        </Show>
        <Show when={searchError()}>
          <div class="error-banner">Error: {searchError()}</div>
        </Show>

        <Show when={searchInspectorResult()}>
          {(item) => (
            <SearchResultInspector
              item={item()}
              index={searchInspectorIndex()}
              total={results().length}
              selected={resultsSelection().has(item().url)}
              downloading={downloadingUrls().has(item().url)}
              onPrev={() => navSearchInspector(-1)}
              onNext={() => navSearchInspector(1)}
              onClose={() => setSearchInspectorUrl(null)}
              onToggleSelected={() => toggleResultSel(item().url)}
              onDownload={() => downloadResult(item())}
            />
          )}
        </Show>

        <Show when={results().length > 0}>
          <div class="batch-bar">
            <div class="batch-info">
              {resultsSelection().size} of {results().length} selected
            </div>
            <div class="batch-actions">
              <label class="download-concurrency">
                <span>Parallel</span>
                <input
                  type="number"
                  min="1"
                  max="32"
                  value={downloadConcurrency()}
                  disabled={busy()}
                  onChange={(e) => setBatchConcurrency(Number(e.currentTarget.value))}
                />
              </label>
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
                return (
                  <div
                    class="card"
                    classList={{
                      selected: selected(),
                      inspecting: searchInspectorUrl() === r.url,
                      downloading: downloadingUrls().has(r.url),
                      [`kind-${r.kind}`]: true,
                    }}
                    onClick={() => toggleResultSel(r.url)}
                    onDblClick={(e) => {
                      e.preventDefault();
                      e.stopPropagation();
                      setSearchInspectorUrl(r.url);
                    }}
                  >
                    <div class="card-image-wrap">
                      <img
                        src={resultPreviewUrl(r)}
                        alt={r.alt || r.query}
                        loading="lazy"
                      />
                      <Show when={r.kind === "video"}>
                        <span class="kind-badge video-badge">▶ {formatDuration(r.duration_secs)}</span>
                      </Show>
                      <Show when={r.kind !== "video" && r.kind !== "photo"}>
                        <span class="kind-badge">{r.kind}</span>
                      </Show>
                      <button
                        type="button"
                        class="card-inspect-btn search-card-inspect-btn"
                        title="Inspect result"
                        onClick={(e) => {
                          e.stopPropagation();
                          setSearchInspectorUrl(r.url);
                        }}
                      >
                        ⓘ
                      </button>
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
                    <Show when={downloadingUrls().has(r.url)}>
                      <div class="card-download-badge">Downloading</div>
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
            media types — Unsplash returns photos only. Open <strong>Filters</strong>{" "}
            to narrow by orientation, color, size, and more.
          </div>
        </Show>
          </div>
        </div>
      </Show>

      <Show when={mode() === "library"}>
        <div class="library-toolbar">
          <input
            type="search"
            placeholder="Filter by query, alt, author or tag…"
            value={filterText()}
            onInput={(e) => setFilterText(e.currentTarget.value)}
            class="library-filter"
          />
          <select
            value={sortField()}
            onChange={(e) => {
              const value = e.currentTarget.value;
              if (isLibrarySortField(value)) setSortField(value);
            }}
          >
            <option value="downloaded_at">Most recent</option>
            <option value="width">Width</option>
            <option value="source">Source</option>
          </select>
          <button type="button" onClick={() => setSortDesc(!sortDesc())}>
            {sortDesc() ? "↓ Desc" : "↑ Asc"}
          </button>
          <button type="button" onClick={refreshLibrary} disabled={libLoading()}>
            {libLoading() ? "Loading…" : "Refresh"}
          </button>
        </div>

        <Show when={libError()}>
          <div class="error-banner">Error: {libError()}</div>
        </Show>
        <Show when={libInfo()}>
          <div class="info-banner">{libInfo()}</div>
        </Show>

        <div class="library-layout">
          <FacetSidebar
            sourceCounts={facetSourceCounts()}
            kindCounts={facetKindCounts()}
            captionCounts={captionCounts()}
            facetSources={facetSources()}
            facetKinds={facetKinds()}
            facetCaption={facetCaption()}
            topics={topicList()}
            facetTopicId={facetTopicId()}
            onTopicChange={setLibraryTopicFilter}
            onToggleSource={(s) => toggleFacet(facetSources(), setFacetSources, s)}
            onToggleKind={(k) => toggleFacet(facetKinds(), setFacetKinds, k)}
            onCaptionChange={setFacetCaption}
            totalImages={images().length}
            totalFiltered={filteredImages().length}
            onClearAll={() => {
              setFacetSources(new Set<string>());
              setFacetKinds(new Set<string>());
              setFacetCaption("any");
              setFilterText("");
              setLibraryTopicFilter(null);
            }}
          />

          <div class="library-main">
            <Show
              when={images().length > 0 || filterText() || facetSources().size > 0 || facetKinds().size > 0}
            >
              <BulkBar
                selectedCount={librarySelection().size}
                visibleCount={filteredImages().length}
                hidden={images().length - filteredImages().length}
                busy={busy()}
                visionBusy={visionBusy()}
                visionCaptionMode={visionCaptionMode()}
                visionCaptionTask={visionCaptionTask()}
                visionCaptionMinChars={visionCaptionMinChars()}
                visionDetectObjects={visionDetectObjects()}
                onVisionCaptionModeChange={setVisionCaptionMode}
                onVisionCaptionTaskChange={setVisionCaptionTask}
                onVisionCaptionMinCharsChange={setVisionCaptionMinChars}
                onVisionDetectObjectsChange={setVisionDetectObjects}
                onSelectAll={selectAllLibrary}
                onClear={clearLibrarySel}
                onDelete={deleteSelected}
                onAddTags={bulkAppendTags}
                onAnalyzeSelected={() => runVisionForIds(Array.from(librarySelection()))}
              />
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
              <div class="grid library-grid">
                <For each={displayedImages()}>
                  {(img) => {
                    const selected = () => librarySelection().has(img.id);
                    return (
                      <div
                        class="card"
                        classList={{
                          selected: selected(),
                          inspecting: inspectorId() === img.id,
                          [`kind-${img.kind}`]: true,
                        }}
                        onClick={(e) => handleCardClick(img.id, e)}
                        onDblClick={() => setInspectorId(img.id)}
                        onContextMenu={(e) => {
                          e.preventDefault();
                          setInspectorId(img.id);
                          void runVisionForIds([img.id]);
                        }}
                        title="Click to select | Shift-click for range | Double-click to inspect | Right-click for AI analyze"
                      >
                        <div class="card-image-wrap">
                          <LibraryThumb id={img.id} alt={img.alt || img.query} />
                          <Show when={img.kind === "video"}>
                            <span class="kind-badge video-badge">▶ {formatDuration(img.duration_secs)}</span>
                          </Show>
                          <Show when={img.kind !== "video" && img.kind !== "photo"}>
                            <span class="kind-badge">{img.kind}</span>
                          </Show>
                          <button
                            type="button"
                            class="card-inspect-btn"
                            title="Open inspector"
                            onClick={(e) => {
                              e.stopPropagation();
                              setInspectorId(img.id);
                            }}
                          >
                            ⓘ
                          </button>
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
              <Show when={displayedImages().length < filteredImages().length}>
                <div class="library-load-more">
                  <button
                    type="button"
                    class="load-more-btn"
                    onClick={() => setLibVisible((v) => v + LIBRARY_PAGE)}
                  >
                    Load more ({displayedImages().length} of {filteredImages().length})
                  </button>
                </div>
              </Show>
            </Show>
          </div>

          <Show when={inspectorImage()}>
            {(img) => (
              <Inspector
                image={img()}
                index={inspectorIndex()}
                total={inspectorTotal()}
                onPrev={() => navInspector(-1)}
                onNext={() => navInspector(1)}
                onClose={() => setInspectorId(null)}
                visionBusy={visionBusy()}
                visionCaptionMode={visionCaptionMode()}
                visionCaptionTask={visionCaptionTask()}
                visionCaptionMinChars={visionCaptionMinChars()}
                visionDetectObjects={visionDetectObjects()}
                onVisionCaptionModeChange={setVisionCaptionMode}
                onVisionCaptionTaskChange={setVisionCaptionTask}
                onVisionCaptionMinCharsChange={setVisionCaptionMinChars}
                onVisionDetectObjectsChange={setVisionDetectObjects}
                onAnalyze={() => runVisionForIds([img().id])}
                onSave={async (patch) => {
                  try {
                    await api.updateImage(img().id, patch);
                    updateImageInPlace(img().id, patch);
                  } catch (e) {
                    setLibError(String(e));
                    throw e; // let the Inspector know the save failed
                  }
                }}
                onDelete={async () => {
                  if (
                    !confirm(
                      `Delete this item? URL will be blocked from re-downloading.`
                    )
                  )
                    return;
                  try {
                    await api.deleteImages([img().id]);
                    dropThumb(img().id);
                    setInspectorId(null);
                    await refreshLibrary();
                  } catch (e) {
                    setLibError(String(e));
                  }
                }}
              />
            )}
          </Show>
        </div>
      </Show>
    </div>
  );
};

const SearchResultInspector: Component<{
  item: SearchResult;
  index: number;
  total: number;
  selected: boolean;
  downloading: boolean;
  onPrev: () => void;
  onNext: () => void;
  onClose: () => void;
  onToggleSelected: () => void;
  onDownload: () => void;
}> = (props) => {
  const isVideo = () => props.item.kind.toLowerCase() === "video";
  const preview = () =>
    isVideo()
      ? resultVideoPreviewUrl(props.item)
      : resultFullUrl(props.item) || resultPreviewUrl(props.item);
  const poster = () => resultPosterUrl(props.item);
  const dimensions = () => {
    if (props.item.width == null || props.item.height == null) return "—";
    return `${props.item.width}×${props.item.height}`;
  };
  const stats = createMemo(() =>
    [
      props.item.views != null ? `${formatStat(props.item.views)} views` : null,
      props.item.likes != null ? `${formatStat(props.item.likes)} likes` : null,
      props.item.downloads != null
        ? `${formatStat(props.item.downloads)} downloads`
        : null,
    ].filter(Boolean)
  );

  return (
    <section class="search-result-inspector">
      <div class="search-result-inspector-header">
        <div class="inspector-nav">
          <button
            type="button"
            class="inspector-nav-btn"
            onClick={props.onPrev}
            disabled={props.total === 0}
            title="Previous"
          >
            ◂
          </button>
          <span class="inspector-pos">
            <Show when={props.index >= 0} fallback={<>—</>}>
              {props.index + 1} / {props.total}
            </Show>
          </span>
          <button
            type="button"
            class="inspector-nav-btn"
            onClick={props.onNext}
            disabled={props.total === 0}
            title="Next"
          >
            ▸
          </button>
        </div>
        <button
          type="button"
          class="inspector-close"
          onClick={props.onClose}
          title="Close"
        >
          ×
        </button>
      </div>

      <div class="search-result-preview">
        <Show
          when={preview()}
          fallback={<div class="search-result-preview-empty">Preview unavailable</div>}
        >
          {(src) => (
            <Show
              when={isVideo()}
              fallback={
                <img
                  class="search-result-media"
                  src={src()}
                  alt={props.item.alt || props.item.query}
                />
              }
            >
              <video
                class="search-result-media search-result-video"
                src={src()}
                poster={poster()}
                controls
                preload="metadata"
                playsinline
              />
            </Show>
          )}
        </Show>
        <Show when={isVideo()}>
          <span class="kind-badge video-badge">
            ▶ {formatDuration(props.item.duration_secs)}
          </span>
        </Show>
      </div>

      <div class="search-result-details">
        <dl class="inspector-meta">
          <dt>Source</dt>
          <dd>{props.item.source}</dd>
          <dt>Kind</dt>
          <dd>{props.item.kind}</dd>
          <dt>Dimensions</dt>
          <dd>{dimensions()}</dd>
          <dt>File size</dt>
          <dd>{formatBytes(props.item.file_size)}</dd>
          <Show when={props.item.author_name}>
            <dt>Author</dt>
            <dd>
              <Show
                when={safeHref(props.item.author_url)}
                fallback={<span>{props.item.author_name}</span>}
              >
                {(href) => (
                  <a href={href()} target="_blank" rel="noreferrer noopener">
                    {props.item.author_name}
                  </a>
                )}
              </Show>
            </dd>
          </Show>
          <dt>Query</dt>
          <dd class="mono-cell">{props.item.query}</dd>
          <Show when={stats().length > 0}>
            <dt>Stats</dt>
            <dd>{stats().join(" · ")}</dd>
          </Show>
          <Show when={props.item.color}>
            <dt>Avg color</dt>
            <dd>
              <span class="color-dot" style={{ background: props.item.color! }} />
              <span class="mono-cell">{props.item.color}</span>
            </dd>
          </Show>
          <Show when={safeHref(props.item.source_page_url)}>
            {(href) => (
              <>
                <dt>Source page</dt>
                <dd>
                  <a href={href()} target="_blank" rel="noreferrer noopener">
                    open
                  </a>
                </dd>
              </>
            )}
          </Show>
        </dl>

        <Show when={props.item.alt}>
          <div class="search-result-caption">{props.item.alt}</div>
        </Show>

        <div class="search-result-actions">
          <button type="button" onClick={props.onToggleSelected}>
            {props.selected ? "Remove from selection" : "Select for download"}
          </button>
          <button
            type="button"
            class="primary"
            onClick={props.onDownload}
            disabled={props.downloading}
          >
            {props.downloading ? "Downloading..." : "Download this"}
          </button>
        </div>
      </div>
    </section>
  );
};

// ---- Library subcomponents ----

const FacetSidebar: Component<{
  sourceCounts: [string, number][];
  kindCounts: [string, number][];
  captionCounts: { captioned: number; uncaptioned: number };
  facetSources: Set<string>;
  facetKinds: Set<string>;
  facetCaption: "any" | "captioned" | "uncaptioned";
  topics: TopicSummary[];
  facetTopicId: string | null;
  onTopicChange: (id: string | null) => void;
  onToggleSource: (s: string) => void;
  onToggleKind: (k: string) => void;
  onCaptionChange: (v: "any" | "captioned" | "uncaptioned") => void;
  totalImages: number;
  totalFiltered: number;
  onClearAll: () => void;
}> = (props) => {
  const topicsWithSaves = () => props.topics.filter((t) => t.saved_count > 0);
  return (
    <aside class="facet-sidebar">
      <div class="facet-header">
        <span class="facet-total">
          {props.totalFiltered}
          <Show when={props.totalFiltered !== props.totalImages}>
            {" of "}
            {props.totalImages}
          </Show>
        </span>
        <button
          type="button"
          class="facet-clear"
          onClick={props.onClearAll}
          disabled={
            props.facetSources.size === 0 &&
            props.facetKinds.size === 0 &&
            props.facetCaption === "any" &&
            props.facetTopicId === null
          }
        >
          Clear
        </button>
      </div>

      <FacetGroup title="Source">
        <Show when={props.sourceCounts.length === 0}>
          <div class="facet-empty">No items yet</div>
        </Show>
        <For each={props.sourceCounts}>
          {([name, count]) => (
            <button
              type="button"
              class="facet-row"
              classList={{ active: props.facetSources.has(name) }}
              onClick={() => props.onToggleSource(name)}
            >
              <span class="facet-row-label">{name}</span>
              <span class="facet-row-count">{count}</span>
            </button>
          )}
        </For>
      </FacetGroup>

      <FacetGroup title="Kind">
        <For each={props.kindCounts}>
          {([name, count]) => (
            <button
              type="button"
              class="facet-row"
              classList={{ active: props.facetKinds.has(name) }}
              onClick={() => props.onToggleKind(name)}
            >
              <span class="facet-row-label">{name}</span>
              <span class="facet-row-count">{count}</span>
            </button>
          )}
        </For>
      </FacetGroup>

      <FacetGroup title="Caption">
        <button
          type="button"
          class="facet-row"
          classList={{ active: props.facetCaption === "captioned" }}
          onClick={() =>
            props.onCaptionChange(
              props.facetCaption === "captioned" ? "any" : "captioned"
            )
          }
        >
          <span class="facet-row-label">Has caption</span>
          <span class="facet-row-count">{props.captionCounts.captioned}</span>
        </button>
        <button
          type="button"
          class="facet-row"
          classList={{ active: props.facetCaption === "uncaptioned" }}
          onClick={() =>
            props.onCaptionChange(
              props.facetCaption === "uncaptioned" ? "any" : "uncaptioned"
            )
          }
        >
          <span class="facet-row-label">No caption</span>
          <span class="facet-row-count">{props.captionCounts.uncaptioned}</span>
        </button>
      </FacetGroup>

      <Show when={topicsWithSaves().length > 0}>
        <FacetGroup title="Topic">
          <For each={topicsWithSaves()}>
            {(t) => (
              <button
                type="button"
                class="facet-row"
                classList={{ active: props.facetTopicId === t.id }}
                onClick={() =>
                  props.onTopicChange(
                    props.facetTopicId === t.id ? null : t.id
                  )
                }
                title={`"${t.query}"`}
              >
                <span class="facet-row-label">
                  <Show when={t.name} fallback={<>"{t.query}"</>}>
                    {t.name}
                  </Show>
                </span>
                <span class="facet-row-count">{t.saved_count}</span>
              </button>
            )}
          </For>
        </FacetGroup>
      </Show>
    </aside>
  );
};

const FacetGroup: Component<{ title: string; children: any }> = (props) => (
  <div class="facet-group">
    <div class="facet-group-title">{props.title}</div>
    <div class="facet-group-body">{props.children}</div>
  </div>
);

const VisionOptionsControls: Component<{
  disabled?: boolean;
  captionMode: VisionCaptionMode;
  captionTask: VisionCaptionTask;
  captionMinChars: number;
  detectObjects: boolean;
  onCaptionModeChange: (value: VisionCaptionMode) => void;
  onCaptionTaskChange: (value: VisionCaptionTask) => void;
  onCaptionMinCharsChange: (value: number) => void;
  onDetectObjectsChange: (value: boolean) => void;
}> = (props) => (
  <div class="vision-options">
    <label class="vision-option">
      <span>Caption</span>
      <select
        value={props.captionMode}
        disabled={props.disabled}
        onChange={(e) =>
          props.onCaptionModeChange(e.currentTarget.value as VisionCaptionMode)
        }
      >
        <option value="missing">Missing only</option>
        <option value="short">Replace short</option>
        <option value="overwrite">Overwrite</option>
        <option value="skip">Tags only</option>
      </select>
    </label>
    <label class="vision-option">
      <span>Detail</span>
      <select
        value={props.captionTask}
        disabled={props.disabled || props.captionMode === "skip"}
        onChange={(e) =>
          props.onCaptionTaskChange(e.currentTarget.value as VisionCaptionTask)
        }
      >
        <option value="caption">Short</option>
        <option value="detailed">Detailed</option>
        <option value="more_detailed">Long</option>
      </select>
    </label>
    <label class="vision-option vision-short-limit">
      <span>Short &lt;</span>
      <input
        type="number"
        min="1"
        max="1000"
        step="1"
        value={props.captionMinChars}
        disabled={props.disabled || props.captionMode !== "short"}
        onInput={(e) => props.onCaptionMinCharsChange(Number(e.currentTarget.value))}
      />
    </label>
    <label class="vision-toggle">
      <input
        type="checkbox"
        checked={props.detectObjects}
        disabled={props.disabled}
        onChange={(e) => props.onDetectObjectsChange(e.currentTarget.checked)}
      />
      <span>Objects</span>
    </label>
  </div>
);

const BulkBar: Component<{
  selectedCount: number;
  visibleCount: number;
  hidden: number;
  busy: boolean;
  visionBusy: boolean;
  visionCaptionMode: VisionCaptionMode;
  visionCaptionTask: VisionCaptionTask;
  visionCaptionMinChars: number;
  visionDetectObjects: boolean;
  onVisionCaptionModeChange: (value: VisionCaptionMode) => void;
  onVisionCaptionTaskChange: (value: VisionCaptionTask) => void;
  onVisionCaptionMinCharsChange: (value: number) => void;
  onVisionDetectObjectsChange: (value: boolean) => void;
  onSelectAll: () => void;
  onClear: () => void;
  onDelete: () => void;
  onAddTags: (raw: string) => Promise<void>;
  onAnalyzeSelected: () => void;
}> = (props) => {
  const [tagInput, setTagInput] = createSignal("");
  const submitTags = async () => {
    const v = tagInput().trim();
    if (!v) return;
    await props.onAddTags(v);
    setTagInput("");
  };
  return (
    <div class="batch-bar">
      <div class="batch-info">
        {props.selectedCount} of {props.visibleCount} selected
        <Show when={props.hidden > 0}>
          <span class="library-filter-hint"> · {props.hidden} hidden</span>
        </Show>
      </div>
      <div class="batch-actions">
        <VisionOptionsControls
          disabled={props.busy}
          captionMode={props.visionCaptionMode}
          captionTask={props.visionCaptionTask}
          captionMinChars={props.visionCaptionMinChars}
          detectObjects={props.visionDetectObjects}
          onCaptionModeChange={props.onVisionCaptionModeChange}
          onCaptionTaskChange={props.onVisionCaptionTaskChange}
          onCaptionMinCharsChange={props.onVisionCaptionMinCharsChange}
          onDetectObjectsChange={props.onVisionDetectObjectsChange}
        />
        <Show when={props.selectedCount > 0}>
          <input
            type="text"
            class="bulk-tag-input"
            placeholder="Add tags (comma-separated)…"
            value={tagInput()}
            onInput={(e) => setTagInput(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                submitTags();
              }
            }}
            disabled={props.busy}
          />
          <button
            type="button"
            onClick={submitTags}
            disabled={props.busy || !tagInput().trim()}
          >
            Tag
          </button>
        </Show>
        <button
          type="button"
          onClick={props.onAnalyzeSelected}
          disabled={props.busy || props.selectedCount === 0}
          title="Run Florence-2 using the selected caption and tag policy"
        >
          {props.visionBusy ? "AI running..." : "AI analyze selected"}
        </button>
        <button
          type="button"
          onClick={props.onSelectAll}
          disabled={props.busy}
        >
          Select all
        </button>
        <button type="button" onClick={props.onClear} disabled={props.busy}>
          Clear
        </button>
        <button
          type="button"
          class="danger"
          onClick={props.onDelete}
          disabled={props.busy || props.selectedCount === 0}
        >
          Delete selected
        </button>
      </div>
    </div>
  );
};

const Inspector: Component<{
  image: Image;
  index: number;
  total: number;
  onPrev: () => void;
  onNext: () => void;
  onClose: () => void;
  visionBusy: boolean;
  visionCaptionMode: VisionCaptionMode;
  visionCaptionTask: VisionCaptionTask;
  visionCaptionMinChars: number;
  visionDetectObjects: boolean;
  onVisionCaptionModeChange: (value: VisionCaptionMode) => void;
  onVisionCaptionTaskChange: (value: VisionCaptionTask) => void;
  onVisionCaptionMinCharsChange: (value: number) => void;
  onVisionDetectObjectsChange: (value: boolean) => void;
  onAnalyze: () => void;
  onSave: (patch: { alt?: string; tags?: string[] }) => Promise<void>;
  onDelete: () => Promise<void>;
}> = (props) => {
  const [alt, setAlt] = createSignal(props.image.alt);
  const [tagsRaw, setTagsRaw] = createSignal(props.image.tags.join(", "));
  const [saving, setSaving] = createSignal(false);
  const [savedTick, setSavedTick] = createSignal(false);

  // The Inspector instance is reused across prev/next navigation (the parent
  // <Show> stays truthy), so reset the edit fields whenever the inspected
  // image changes — otherwise a Save writes the previous image's edits onto
  // the newly-shown image's row.
  createEffect(
    on(
      () => [props.image.id, props.image.alt, props.image.tags.join("\u0000")],
      () => {
        setAlt(props.image.alt);
        setTagsRaw(props.image.tags.join(", "));
        setSavedTick(false);
      },
      { defer: true }
    )
  );

  const dirty = () =>
    alt() !== props.image.alt ||
    tagsRaw() !==
      props.image.tags.join(", ");

  const canAnalyze = () =>
    props.image.kind.toLowerCase() !== "video" &&
    !props.image.preview_only &&
    !!props.image.path;

  const save = async () => {
    setSaving(true);
    try {
      const tags = tagsRaw()
        .split(",")
        .map((t) => t.trim())
        .filter((t) => t.length > 0);
      await props.onSave({ alt: alt(), tags });
      setSavedTick(true);
      setTimeout(() => setSavedTick(false), 1500);
    } finally {
      setSaving(false);
    }
  };

  const fmtBytes = (n: number | null) => {
    if (n == null) return "—";
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / 1024 / 1024).toFixed(1)} MB`;
  };

  return (
    <aside class="inspector">
      <div class="inspector-header">
        <div class="inspector-nav">
          <button
            type="button"
            class="inspector-nav-btn"
            onClick={props.onPrev}
            disabled={props.total === 0}
            title="Previous (← / k)"
          >
            ◂
          </button>
          <span class="inspector-pos">
            <Show when={props.index >= 0} fallback={<>—</>}>
              {props.index + 1} / {props.total}
            </Show>
          </span>
          <button
            type="button"
            class="inspector-nav-btn"
            onClick={props.onNext}
            disabled={props.total === 0}
            title="Next (→ / j)"
          >
            ▸
          </button>
        </div>
        <button type="button" class="inspector-close" onClick={props.onClose} title="Close (Esc)">
          ✕
        </button>
      </div>

      <InspectorPreview image={props.image} />

      <div class="inspector-details">
      <dl class="inspector-meta">
        <dt>Source</dt>
        <dd>{props.image.source}</dd>
        <dt>Kind</dt>
        <dd>{props.image.kind}</dd>
        <dt>Dimensions</dt>
        <dd>
          {props.image.width}×{props.image.height}
        </dd>
        <dt>File size</dt>
        <dd>{fmtBytes(props.image.file_size)}</dd>
        <Show when={props.image.author_name}>
          <dt>Author</dt>
          <dd>
            <Show
              when={safeHref(props.image.author_url)}
              fallback={<span>{props.image.author_name}</span>}
            >
              {(href) => (
                <a href={href()} target="_blank" rel="noreferrer noopener">
                  {props.image.author_name}
                </a>
              )}
            </Show>
          </dd>
        </Show>
        <dt>Query</dt>
        <dd class="mono-cell">{props.image.query}</dd>
        <dt>Downloaded</dt>
        <dd class="mono-cell">{props.image.downloaded_at}</dd>
        <Show when={safeHref(props.image.source_page_url)}>
          {(href) => (
            <>
              <dt>Source page</dt>
              <dd>
                <a
                  href={href()}
                  target="_blank"
                  rel="noreferrer noopener"
                  class="mono-cell"
                >
                  open ↗
                </a>
              </dd>
            </>
          )}
        </Show>
        <Show when={props.image.color}>
          <dt>Avg color</dt>
          <dd>
            <span
              class="color-dot"
              style={{ background: props.image.color! }}
            />
            <span class="mono-cell">{props.image.color}</span>
          </dd>
        </Show>
        <dt>AI captioned</dt>
        <dd>{props.image.vision_processed ? "yes" : "no"}</dd>
      </dl>

      <div class="inspector-edit">
        <label class="inspector-field">
          <span>Caption (alt)</span>
          <textarea
            rows="3"
            value={alt()}
            onInput={(e) => setAlt(e.currentTarget.value)}
          />
        </label>
        <label class="inspector-field">
          <span>Tags (comma-separated)</span>
          <textarea
            rows="2"
            value={tagsRaw()}
            onInput={(e) => setTagsRaw(e.currentTarget.value)}
          />
        </label>
        <VisionOptionsControls
          disabled={saving()}
          captionMode={props.visionCaptionMode}
          captionTask={props.visionCaptionTask}
          captionMinChars={props.visionCaptionMinChars}
          detectObjects={props.visionDetectObjects}
          onCaptionModeChange={props.onVisionCaptionModeChange}
          onCaptionTaskChange={props.onVisionCaptionTaskChange}
          onCaptionMinCharsChange={props.onVisionCaptionMinCharsChange}
          onDetectObjectsChange={props.onVisionDetectObjectsChange}
        />
        <div class="inspector-actions">
          <button
            type="button"
            onClick={props.onAnalyze}
            disabled={saving() || props.visionBusy || !canAnalyze()}
            title={
              canAnalyze()
                ? "Run Florence-2 using the selected caption and tag policy"
                : "Florence-2 analyzes downloaded still images only"
            }
          >
            {props.visionBusy ? "AI running..." : "AI analyze"}
          </button>
          <button
            type="button"
            class="primary"
            onClick={save}
            disabled={saving() || !dirty()}
          >
            {saving() ? "Saving…" : savedTick() ? "Saved ✓" : "Save"}
          </button>
          <button
            type="button"
            class="danger"
            onClick={props.onDelete}
            disabled={saving()}
          >
            Delete
          </button>
        </div>
      </div>
      </div>
    </aside>
  );
};

const InspectorPreview: Component<{ image: Image }> = (props) => {
  const [url, setUrl] = createSignal<string | null>(null);
  const [posterUrl, setPosterUrl] = createSignal<string | null>(null);
  const [error, setError] = createSignal(false);

  createEffect(() => {
    const id = props.image.id;
    const isVideo = props.image.kind.toLowerCase() === "video";
    let cancelled = false;

    setUrl(null);
    setPosterUrl(null);
    setError(false);

    (async () => {
      try {
        if (isVideo) {
          const nextPosterUrl = await getThumbUrl(id).catch(() => null);
          if (!cancelled && props.image.id === id) setPosterUrl(nextPosterUrl);
          const nextUrl = await getMediaFileUrl(id);
          if (!cancelled && props.image.id === id) setUrl(nextUrl);
        } else {
          const nextUrl = await getImageUrl(id).catch(() => getThumbUrl(id));
          if (!cancelled && props.image.id === id) setUrl(nextUrl);
        }
      } catch {
        if (!cancelled && props.image.id === id) setError(true);
      }
    })();

    onCleanup(() => {
      cancelled = true;
    });
  });

  return (
    <div
      class="inspector-preview"
      classList={{ loading: !url() && !error(), failed: error() }}
    >
      <Show
        when={url()}
        fallback={
          <Show
            when={error()}
            fallback={<div class="inspector-preview-placeholder" aria-hidden="true" />}
          >
            <Show
              when={posterUrl()}
              fallback={<div class="inspector-preview-error">Preview unavailable</div>}
            >
              {(poster) => (
                <>
                  <img
                    class="inspector-media inspector-media-muted"
                    src={poster()}
                    alt={props.image.alt || props.image.query}
                  />
                  <div class="inspector-preview-error inspector-preview-error-overlay">
                    Video file unavailable
                  </div>
                </>
              )}
            </Show>
          </Show>
        }
      >
        {(src) => (
          <Show
            when={props.image.kind.toLowerCase() === "video"}
            fallback={
              <img
                class="inspector-media"
                src={src()}
                alt={props.image.alt || props.image.query}
              />
            }
          >
            <video
              class="inspector-media inspector-video"
              src={src()}
              poster={posterUrl() ?? undefined}
              controls
              preload="metadata"
              playsinline
              onError={() => {
                setUrl(null);
                setError(true);
              }}
            />
          </Show>
        )}
      </Show>
      <Show when={props.image.kind.toLowerCase() === "video"}>
        <span class="kind-badge video-badge">
          ▶ {formatDuration(props.image.duration_secs)}
        </span>
      </Show>
    </div>
  );
};

const SearchBar: Component<{
  query: string;
  onQueryChange: (v: string) => void;
  enabledSources: Set<SourceId>;
  onToggleSource: (s: SourceId) => void;
  count: number;
  onCountChange: (v: number) => void;
  kind: SearchKindParam;
  onKindChange: (v: SearchKindParam) => void;
  searching: boolean;
  onSearch: (e?: Event) => void;
  filtersOpen: boolean;
  onToggleFilters: () => void;
  activeFilterCount: number;
  budget: BudgetBreakdown;
  quota: QuotaSnapshot | null;
}> = (props) => {
  return (
    <form class="search-bar-v2" onSubmit={props.onSearch}>
      <div class="search-row-primary">
        <input
          type="search"
          class="search-input"
          placeholder="Search Pixabay + Pexels + Unsplash…"
          value={props.query}
          onInput={(e) => props.onQueryChange(e.currentTarget.value)}
          autofocus
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
        <button
          type="submit"
          class="primary search-submit"
          disabled={props.searching || !props.query.trim()}
        >
          {props.searching ? "Searching…" : "Search"}
        </button>
      </div>

      <div class="search-row-secondary">
        <div class="source-chips">
          <For each={SOURCE_IDS}>
            {(s) => {
              const enabled = () => props.enabledSources.has(s);
              return (
                <button
                  type="button"
                  class="source-chip"
                  classList={{ active: enabled() }}
                  onClick={() => props.onToggleSource(s)}
                  title={`Toggle ${s}`}
                >
                  <span
                    class="source-chip-dot"
                    classList={{ on: enabled() }}
                  />
                  {s.charAt(0).toUpperCase() + s.slice(1)}
                </button>
              );
            }}
          </For>
        </div>

        <label class="results-count">
          <span class="results-count-label">Results / source</span>
          <input
            type="number"
            min="1"
            max="500"
            step="1"
            value={props.count}
            onInput={(e) =>
              props.onCountChange(Number(e.currentTarget.value) || 1)
            }
          />
        </label>

        <button
          type="button"
          class="filters-toggle"
          classList={{ open: props.filtersOpen, "has-active": props.activeFilterCount > 0 }}
          onClick={props.onToggleFilters}
        >
          <span class="filters-caret">{props.filtersOpen ? "▾" : "▸"}</span>
          Filters
          <Show when={props.activeFilterCount > 0}>
            <span class="filters-count-badge">{props.activeFilterCount}</span>
          </Show>
        </button>
      </div>

      <div class="search-row-meter">
        <BudgetPreview budget={props.budget} />
        <Show when={props.quota}>
          <QuotaChips
            quota={props.quota!}
            enabled={props.enabledSources}
          />
        </Show>
      </div>
    </form>
  );
};

const TopicsSidebar: Component<{
  topics: TopicSummary[];
  currentId: string | null;
  onSelect: (id: string) => void;
  onRename: (id: string, name: string | null) => void;
  onDelete: (id: string) => void;
  onNew: () => void;
  onClose: () => void;
}> = (props) => {
  const [editingId, setEditingId] = createSignal<string | null>(null);
  const [draftName, setDraftName] = createSignal("");
  const [filter, setFilter] = createSignal("");

  const visible = () => {
    const f = filter().trim().toLowerCase();
    if (!f) return props.topics;
    return props.topics.filter(
      (t) =>
        (t.name || "").toLowerCase().includes(f) ||
        t.query.toLowerCase().includes(f)
    );
  };

  const startEdit = (t: TopicSummary) => {
    setEditingId(t.id);
    setDraftName(t.name ?? t.query);
  };
  const commitEdit = () => {
    const id = editingId();
    if (!id) return;
    const v = draftName().trim();
    props.onRename(id, v.length === 0 ? null : v);
    setEditingId(null);
  };

  return (
    <aside class="topics-sidebar">
      <div class="topics-sidebar-header">
        <h3>Topics</h3>
        <div class="topics-sidebar-header-actions">
          <button
            type="button"
            class="topics-new"
            onClick={props.onNew}
            title="Clear current topic so the next Search creates a fresh one"
          >
            + New
          </button>
          <button
            type="button"
            class="topics-close"
            onClick={props.onClose}
            title="Hide sidebar"
          >
            ◂
          </button>
        </div>
      </div>

      <Show when={props.topics.length > 6}>
        <input
          type="search"
          class="topics-filter"
          placeholder="Filter topics…"
          value={filter()}
          onInput={(e) => setFilter(e.currentTarget.value)}
        />
      </Show>

      <Show
        when={props.topics.length > 0}
        fallback={
          <div class="topics-empty">
            No topics yet. Run a search to create one.
          </div>
        }
      >
        <div class="topics-list">
          <For each={visible()}>
            {(t) => {
              const active = () => props.currentId === t.id;
              return (
                <div
                  class="topic-item"
                  classList={{ active: active() }}
                  onClick={() => {
                    if (editingId() === t.id) return;
                    props.onSelect(t.id);
                  }}
                >
                  <Show
                    when={editingId() === t.id}
                    fallback={
                      <span
                        class="topic-item-name"
                        title={`"${t.query}" · ${t.kind}`}
                        onDblClick={(e) => {
                          e.stopPropagation();
                          startEdit(t);
                        }}
                      >
                        <Show
                          when={t.name}
                          fallback={<>"{t.query}"</>}
                        >
                          {t.name}
                          <span class="topic-item-query">"{t.query}"</span>
                        </Show>
                      </span>
                    }
                  >
                    <input
                      type="text"
                      class="topic-item-rename"
                      autofocus
                      value={draftName()}
                      onInput={(e) => setDraftName(e.currentTarget.value)}
                      onClick={(e) => e.stopPropagation()}
                      onBlur={commitEdit}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") commitEdit();
                        else if (e.key === "Escape") setEditingId(null);
                      }}
                    />
                  </Show>
                  <div class="topic-item-meta">
                    <span class="topic-item-stats">
                      <strong>{t.saved_count}</strong>
                      <span class="topic-item-stat-hint">
                        /{t.seen_count}
                      </span>
                    </span>
                    <span class="topic-item-kind">{t.kind}</span>
                  </div>
                  <div class="topic-item-actions">
                    <button
                      type="button"
                      class="topic-item-edit"
                      onClick={(e) => {
                        e.stopPropagation();
                        startEdit(t);
                      }}
                      title="Rename"
                    >
                      ✎
                    </button>
                    <button
                      type="button"
                      class="topic-item-delete"
                      onClick={(e) => {
                        e.stopPropagation();
                        props.onDelete(t.id);
                      }}
                      title="Delete topic (saved items kept)"
                    >
                      ✕
                    </button>
                  </div>
                </div>
              );
            }}
          </For>
        </div>
      </Show>
    </aside>
  );
};

const TopicBar: Component<{
  topic: Topic;
  status: TopicStatus | null;
  lastProgress: TopicProgress[] | null;
  onGetMore: () => void;
  onReset: () => void;
  onClear: () => void;
  busy: boolean;
  fillTarget: number;
  onFillTargetChange: (n: number) => void;
  fillRunning: boolean;
  fillProgress: {
    gathered: number;
    rounds: number;
    apiCalls: number;
    exhaustedSources: string[];
  };
  onFillN: () => void;
  onFillStop: () => void;
}> = (props) => {
  const allExhausted = () => {
    const cs = props.status?.cursors ?? [];
    return cs.length > 0 && cs.every((c) => c.last_status === "empty");
  };
  const cursorChips = () => props.status?.cursors ?? [];

  const lastSummary = () => {
    const lp = props.lastProgress;
    if (!lp || lp.length === 0) return null;
    const total_kept = lp.reduce((a, b) => a + b.kept_count, 0);
    const total_raw = lp.reduce((a, b) => a + b.raw_count, 0);
    const dedup = total_raw - total_kept;
    const exhausted = lp.filter((p) => p.status === "empty").length;
    return { total_kept, total_raw, dedup, exhausted };
  };

  return (
    <div class="topic-bar">
      <div class="topic-row-top">
        <div class="topic-id-block">
          <span class="topic-label">Topic</span>
          <span class="topic-query" title={`Created ${props.topic.created_at}`}>
            "{props.topic.query}"
          </span>
          <span class="topic-kind-pill">{props.topic.kind}</span>
        </div>
        <div class="topic-actions">
          <Show
            when={!props.fillRunning}
            fallback={
              <>
                <span class="fill-running">
                  Filling… {props.fillProgress.gathered} /{" "}
                  {props.fillTarget} ·{" "}
                  {props.fillProgress.rounds} rounds ·{" "}
                  {props.fillProgress.apiCalls} calls
                </span>
                <button
                  type="button"
                  class="danger"
                  onClick={props.onFillStop}
                  title="Stop fill loop after the current round"
                >
                  Stop
                </button>
              </>
            }
          >
            <input
              type="number"
              class="fill-target-input"
              min="1"
              max="2000"
              step="1"
              value={props.fillTarget}
              onInput={(e) =>
                props.onFillTargetChange(
                  Math.max(1, Math.min(2000, Number(e.currentTarget.value) || 1))
                )
              }
              title="Number of new uniques to gather"
              disabled={props.busy}
            />
            <button
              type="button"
              onClick={props.onFillN}
              disabled={props.busy || allExhausted()}
              title={
                allExhausted()
                  ? "All sources exhausted — Reset to start over"
                  : `Loop Get-more rounds until ${props.fillTarget} new items collected or all sources exhaust`
              }
            >
              Fill
            </button>
            <button
              type="button"
              class="primary"
              onClick={props.onGetMore}
              disabled={props.busy || allExhausted()}
              title={
                allExhausted()
                  ? "All sources exhausted — Reset to start over"
                  : "Fetch the next page from each active source"
              }
            >
              {props.busy ? "Fetching…" : "Get more"}
            </button>
            <button
              type="button"
              onClick={props.onReset}
              disabled={props.busy}
              title="Wipe pagination + seen list. Next round starts at page 1."
            >
              Reset
            </button>
            <button
              type="button"
              class="topic-clear"
              onClick={props.onClear}
              disabled={props.busy}
              title="Forget the current topic without deleting it"
            >
              ✕
            </button>
          </Show>
        </div>
      </div>

      <Show when={props.fillRunning}>
        <div class="fill-progress-bar">
          <div
            class="fill-progress-fill"
            style={{
              width: `${Math.min(100, (props.fillProgress.gathered / Math.max(1, props.fillTarget)) * 100)}%`,
            }}
          />
        </div>
      </Show>

      <Show when={props.status}>
        <div class="topic-stats">
          <span class="topic-stat">
            <strong>{props.status!.saved_count}</strong> saved
          </span>
          <span class="topic-stat">
            <strong>{props.status!.seen_count}</strong> seen
          </span>
          <span class="topic-stat-sep">·</span>
          <For each={cursorChips()}>
            {(c) => (
              <span
                class="topic-cursor-chip"
                data-status={c.last_status}
                title={
                  c.last_status === "empty"
                    ? `${c.source} (${c.media_kind}) — exhausted`
                    : `${c.source} (${c.media_kind}) — next page ${c.next_page}, last fetch: ${c.last_status}`
                }
              >
                <span class="topic-cursor-name">
                  {c.source}
                  <Show when={c.media_kind === "video"}>
                    <span class="topic-cursor-kind"> ▶</span>
                  </Show>
                </span>
                <Show
                  when={c.last_status !== "empty"}
                  fallback={<span class="topic-cursor-state">∅</span>}
                >
                  <span class="topic-cursor-state">p{c.next_page}</span>
                </Show>
              </span>
            )}
          </For>
        </div>
      </Show>

      <Show when={lastSummary()}>
        {(s) => (
          <div class="topic-last-round">
            Last round: {s().total_raw} returned · {s().total_kept} new
            <Show when={s().dedup > 0}>
              <span class="topic-dedup">
                {" "}· {s().dedup} skipped (already saved or seen)
              </span>
            </Show>
            <Show when={s().exhausted > 0}>
              <span class="topic-exhausted">
                {" "}· {s().exhausted} source{s().exhausted === 1 ? "" : "s"} exhausted
              </span>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
};

const BudgetPreview: Component<{ budget: BudgetBreakdown }> = (props) => {
  return (
    <div class="budget-preview" title="Estimated API calls for one Search press">
      <span class="budget-icon">⚡</span>
      <span class="budget-total">≈ {props.budget.total} API call{props.budget.total === 1 ? "" : "s"}</span>
      <Show when={props.budget.perSource.length > 0}>
        <span class="budget-sep">·</span>
        <span class="budget-breakdown">
          <For each={props.budget.perSource}>
            {(s, i) => (
              <>
                <Show when={i() > 0}>
                  <span class="budget-dot"> · </span>
                </Show>
                <span class="budget-piece">
                  {s.id} {s.calls > 0 ? s.calls : "0"}
                  <Show when={s.note}>
                    <span class="budget-note"> ({s.note})</span>
                  </Show>
                </span>
              </>
            )}
          </For>
        </span>
      </Show>
    </div>
  );
};

const QuotaChips: Component<{
  quota: QuotaSnapshot;
  enabled: Set<SourceId>;
}> = (props) => {
  return (
    <div class="quota-chips">
      <For each={SOURCE_IDS}>
        {(id) => (
          <QuotaChip
            id={id}
            slot={props.quota[id]}
            dim={!props.enabled.has(id)}
          />
        )}
      </For>
    </div>
  );
};

const QuotaChip: Component<{ id: SourceId; slot: QuotaSlot; dim: boolean }> = (
  props
) => {
  const fmtReset = () => {
    if (props.slot.reset_epoch == null) return null;
    const now = Math.floor(Date.now() / 1000);
    const diff = props.slot.reset_epoch - now;
    if (diff <= 0) return null;
    if (diff < 60) return `${diff}s`;
    if (diff < 3600) return `${Math.round(diff / 60)}m`;
    if (diff < 86400) return `${Math.round(diff / 3600)}h`;
    return null;
  };
  const ratio = () => {
    const r = props.slot.remaining;
    const l = props.slot.limit;
    if (r == null || l == null || l === 0) return null;
    return r / l;
  };
  const tone = () => {
    const r = ratio();
    if (r == null) return "neutral";
    if (r < 0.1) return "danger";
    if (r < 0.25) return "warn";
    return "ok";
  };
  const label = () => {
    if (props.slot.remaining == null) return "—";
    if (props.slot.limit != null) {
      return `${props.slot.remaining}/${props.slot.limit}`;
    }
    return `${props.slot.remaining}`;
  };
  const title = () => {
    const parts: string[] = [];
    if (props.slot.last_status != null) parts.push(`HTTP ${props.slot.last_status}`);
    if (props.slot.total_calls > 0) parts.push(`${props.slot.total_calls} calls this session`);
    const r = fmtReset();
    if (r) parts.push(`resets in ${r}`);
    if (parts.length === 0) parts.push("No calls yet — run a search");
    return parts.join(" · ");
  };
  return (
    <div
      class="quota-chip"
      data-tone={tone()}
      classList={{ dim: props.dim }}
      title={title()}
    >
      <span class="quota-chip-name">{props.id}</span>
      <span class="quota-chip-value">{label()}</span>
      <Show when={fmtReset()}>
        <span class="quota-chip-reset">↻{fmtReset()}</span>
      </Show>
    </div>
  );
};

const FiltersPanel: Component<{
  filters: SearchFilters;
  onChange: <K extends keyof SearchFilters>(key: K, value: SearchFilters[K]) => void;
  onClear: () => void;
  kind: SearchKindParam;
}> = (props) => {
  const orientation = () => props.filters.orientation || "any";
  const order = () => props.filters.order || "popular";
  return (
    <div class="filters-panel">
      <div class="filters-row">
        <div class="filter-group">
          <span class="filter-label">Orientation</span>
          <div class="chip-row">
            <For each={["any", "landscape", "portrait", "square"]}>
              {(o) => (
                <button
                  type="button"
                  class="chip"
                  classList={{ active: orientation() === o }}
                  onClick={() => props.onChange("orientation", o)}
                >
                  {o.charAt(0).toUpperCase() + o.slice(1)}
                </button>
              )}
            </For>
          </div>
        </div>

        <div class="filter-group">
          <span class="filter-label">Order</span>
          <div class="chip-row">
            <For each={[
              { id: "popular", label: "Popular" },
              { id: "latest", label: "Latest" },
              { id: "relevant", label: "Relevant" },
            ]}>
              {(o) => (
                <button
                  type="button"
                  class="chip"
                  classList={{ active: order() === o.id }}
                  onClick={() => props.onChange("order", o.id)}
                >
                  {o.label}
                </button>
              )}
            </For>
          </div>
        </div>

        <div class="filter-group">
          <span class="filter-label">
            Size <span class="filter-hint">(Pexels)</span>
          </span>
          <div class="chip-row">
            <button
              type="button"
              class="chip"
              classList={{ active: !props.filters.size }}
              onClick={() => props.onChange("size", undefined)}
            >
              Any
            </button>
            <For each={["small", "medium", "large"]}>
              {(s) => (
                <button
                  type="button"
                  class="chip"
                  classList={{ active: props.filters.size === s }}
                  onClick={() => props.onChange("size", s)}
                  title={
                    s === "large"
                      ? "≥ 24MP"
                      : s === "medium"
                        ? "≥ 12MP"
                        : "≥ 4MP"
                  }
                >
                  {s.charAt(0).toUpperCase() + s.slice(1)}
                </button>
              )}
            </For>
          </div>
        </div>
      </div>

      <div class="filters-row">
        <div class="filter-group filter-group-wide">
          <span class="filter-label">Color</span>
          <div class="color-swatches">
            <button
              type="button"
              class="color-swatch swatch-any"
              classList={{ active: !props.filters.color }}
              onClick={() => props.onChange("color", undefined)}
              title="Any color"
            >
              <span class="swatch-x">×</span>
            </button>
            <For each={COLOR_SWATCHES}>
              {(c) => (
                <button
                  type="button"
                  class="color-swatch"
                  classList={{ active: props.filters.color === c.id }}
                  onClick={() =>
                    props.onChange(
                      "color",
                      props.filters.color === c.id ? undefined : c.id
                    )
                  }
                  style={{ background: c.hex }}
                  title={c.label}
                />
              )}
            </For>
          </div>
        </div>
      </div>

      <div class="filters-row">
        <div class="filter-group">
          <span class="filter-label">Min size</span>
          <div class="dim-row">
            <label class="dim-input">
              <span>W</span>
              <input
                type="number"
                min="0"
                step="1"
                value={props.filters.min_width ?? ""}
                placeholder="any"
                onInput={(e) => {
                  const v = e.currentTarget.value;
                  props.onChange("min_width", v ? Number(v) : undefined);
                }}
              />
              <em>px</em>
            </label>
            <label class="dim-input">
              <span>H</span>
              <input
                type="number"
                min="0"
                step="1"
                value={props.filters.min_height ?? ""}
                placeholder="any"
                onInput={(e) => {
                  const v = e.currentTarget.value;
                  props.onChange("min_height", v ? Number(v) : undefined);
                }}
              />
              <em>px</em>
            </label>
          </div>
        </div>

        <div class="filter-group">
          <span class="filter-label">
            Category <span class="filter-hint">(Pixabay)</span>
          </span>
          <select
            class="filter-select"
            value={props.filters.category ?? ""}
            onChange={(e) =>
              props.onChange(
                "category",
                e.currentTarget.value || undefined
              )
            }
          >
            <option value="">Any</option>
            <For each={PIXABAY_CATEGORIES}>
              {(c) => (
                <option value={c}>
                  {c.charAt(0).toUpperCase() + c.slice(1)}
                </option>
              )}
            </For>
          </select>
        </div>

        <Show when={props.kind !== "photo"}>
          <div class="filter-group">
            <span class="filter-label">
              Video type <span class="filter-hint">(Pixabay)</span>
            </span>
            <div class="chip-row">
              <For each={[
                { id: undefined, label: "Any" },
                { id: "film", label: "Film" },
                { id: "animation", label: "Animation" },
              ]}>
                {(v) => (
                  <button
                    type="button"
                    class="chip"
                    classList={{
                      active: (props.filters.video_type ?? undefined) === v.id,
                    }}
                    onClick={() => props.onChange("video_type", v.id)}
                  >
                    {v.label}
                  </button>
                )}
              </For>
            </div>
          </div>
        </Show>
      </div>

      <div class="filters-row filters-row-toggles">
        <label class="toggle">
          <input
            type="checkbox"
            checked={props.filters.safesearch ?? true}
            onChange={(e) =>
              props.onChange("safesearch", e.currentTarget.checked)
            }
          />
          <span>Safe search</span>
        </label>
        <label class="toggle">
          <input
            type="checkbox"
            checked={props.filters.editors_choice ?? false}
            onChange={(e) =>
              props.onChange("editors_choice", e.currentTarget.checked)
            }
          />
          <span>
            Editor's choice <em class="filter-hint">(Pixabay)</em>
          </span>
        </label>
        <label class="toggle">
          <input
            type="checkbox"
            checked={props.filters.exclude_ai ?? false}
            onChange={(e) =>
              props.onChange("exclude_ai", e.currentTarget.checked)
            }
          />
          <span>Hide AI-generated</span>
        </label>
        <button type="button" class="filters-clear" onClick={props.onClear}>
          Clear filters
        </button>
      </div>
    </div>
  );
};

// ---- localStorage helpers ----

function loadFilters(): SearchFilters {
  try {
    const raw = localStorage.getItem(FILTERS_KEY);
    if (!raw) return { ...DEFAULT_FILTERS };
    const parsed = JSON.parse(raw);
    return { ...DEFAULT_FILTERS, ...parsed };
  } catch {
    return { ...DEFAULT_FILTERS };
  }
}
function saveFilters(f: SearchFilters) {
  try {
    localStorage.setItem(FILTERS_KEY, JSON.stringify(f));
  } catch {}
}
function loadSourceSet(): Set<SourceId> {
  try {
    const raw = localStorage.getItem(SOURCES_KEY);
    if (!raw) return new Set<SourceId>(SOURCE_IDS);
    const arr = JSON.parse(raw) as SourceId[];
    const set = new Set<SourceId>(arr.filter((s) => SOURCE_IDS.includes(s)));
    if (set.size === 0) return new Set<SourceId>(SOURCE_IDS);
    return set;
  } catch {
    return new Set<SourceId>(SOURCE_IDS);
  }
}
function saveSourceSet(set: Set<SourceId>) {
  try {
    localStorage.setItem(SOURCES_KEY, JSON.stringify(Array.from(set)));
  } catch {}
}
function loadCount(): number {
  try {
    const raw = localStorage.getItem(COUNT_KEY);
    if (!raw) return 80;
    const n = Number(raw);
    if (!Number.isFinite(n) || n < 1) return 80;
    return Math.min(500, Math.round(n));
  } catch {
    return 80;
  }
}
function saveCount(n: number) {
  try {
    localStorage.setItem(COUNT_KEY, String(n));
  } catch {}
}

function loadDownloadConcurrency(): number {
  try {
    const raw = localStorage.getItem(DOWNLOAD_CONCURRENCY_KEY);
    if (!raw) return 8;
    const n = Number(raw);
    if (!Number.isFinite(n)) return 8;
    return clampDownloadConcurrency(n);
  } catch {
    return 8;
  }
}

function saveDownloadConcurrency(n: number) {
  try {
    localStorage.setItem(DOWNLOAD_CONCURRENCY_KEY, String(clampDownloadConcurrency(n)));
  } catch {}
}

function loadVisionCaptionMode(): VisionCaptionMode {
  try {
    const raw = localStorage.getItem(VISION_CAPTION_MODE_KEY);
    if (
      raw === "missing" ||
      raw === "short" ||
      raw === "overwrite" ||
      raw === "skip"
    ) {
      return raw;
    }
  } catch {}
  return "missing";
}

function saveVisionCaptionMode(mode: VisionCaptionMode) {
  try {
    localStorage.setItem(VISION_CAPTION_MODE_KEY, mode);
  } catch {}
}

function loadVisionCaptionTask(): VisionCaptionTask {
  try {
    const raw = localStorage.getItem(VISION_CAPTION_TASK_KEY);
    if (raw === "caption" || raw === "detailed" || raw === "more_detailed") {
      return raw;
    }
  } catch {}
  return "detailed";
}

function saveVisionCaptionTask(task: VisionCaptionTask) {
  try {
    localStorage.setItem(VISION_CAPTION_TASK_KEY, task);
  } catch {}
}

function loadVisionCaptionMinChars(): number {
  try {
    const raw = localStorage.getItem(VISION_CAPTION_MIN_CHARS_KEY);
    if (!raw) return DEFAULT_VISION_CAPTION_MIN_CHARS;
    const n = Number(raw);
    if (!Number.isFinite(n)) return DEFAULT_VISION_CAPTION_MIN_CHARS;
    return clampVisionCaptionMinChars(n);
  } catch {
    return DEFAULT_VISION_CAPTION_MIN_CHARS;
  }
}

function saveVisionCaptionMinChars(n: number) {
  try {
    localStorage.setItem(
      VISION_CAPTION_MIN_CHARS_KEY,
      String(clampVisionCaptionMinChars(n))
    );
  } catch {}
}

function loadVisionDetectObjects(): boolean {
  try {
    const raw = localStorage.getItem(VISION_DETECT_OBJECTS_KEY);
    if (raw === "false") return false;
    if (raw === "true") return true;
  } catch {}
  return true;
}

function saveVisionDetectObjects(value: boolean) {
  try {
    localStorage.setItem(VISION_DETECT_OBJECTS_KEY, String(value));
  } catch {}
}

export default ImagesTab;
