import { createSignal, onCleanup, onMount, type Component } from "solid-js";
import { getThumbUrl } from "../lib/thumbCache";

type Props = {
  id: string;
  alt?: string;
};

const LibraryThumb: Component<Props> = (props) => {
  const [url, setUrl] = createSignal<string | null>(null);
  const [error, setError] = createSignal(false);
  let imgEl: HTMLImageElement | undefined;
  let observer: IntersectionObserver | undefined;
  let loaded = false;

  const load = async () => {
    if (loaded) return;
    loaded = true;
    try {
      setUrl(await getThumbUrl(props.id));
    } catch {
      setError(true);
    }
  };

  onMount(() => {
    if (!imgEl) return;
    observer = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            load();
            observer?.disconnect();
          }
        }
      },
      { rootMargin: "200px" }
    );
    observer.observe(imgEl);
  });
  onCleanup(() => observer?.disconnect());

  return (
    <img
      ref={imgEl}
      src={url() ?? ""}
      alt={props.alt ?? ""}
      classList={{ "thumb-loaded": !!url(), "thumb-error": error() }}
      style={{
        width: "100%",
        height: "100%",
        "object-fit": "cover",
        background: "var(--bg-input)",
        opacity: url() ? 1 : 0.2,
        transition: "opacity 200ms",
      }}
    />
  );
};

export default LibraryThumb;
