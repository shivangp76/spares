import { useEffect, useMemo, useState, type ReactNode } from "react";
import { marked } from "marked";
import { fetchFile, getNote } from "../api/client";
import { useAuthedFileUrl } from "../hooks/useAuthedFileUrl";
import { usePendingRender } from "../local/copy";
import { usePandocHtml } from "../local/pandoc";
import { deviceMediaFile } from "../local/transport";
import type { NoteResponse } from "../types/spares";
import { errorMessage, imageMimeType } from "../utils";
import { markdownCardHtml } from "../markdownCard";
import HtmlViewer from "./HtmlViewer";
import PdfViewer from "./PdfViewer";
import TypstViewer from "./TypstViewer";
import { useMediaQuery } from "../hooks/useMediaQuery";
import { useCardZoom, usePagesPerRow } from "../preferences";

interface Props {
  path: string;
  /** Raw source for parsers compiled in the browser. Takes precedence over `path`. */
  source?: string;
  /** HTML for web clients, shown instead of the rendered file at `path`. */
  webPath?: string;
  /** The card's Markdown source, on a device, shown while the server hasn't rendered its note's latest change. */
  deviceSource?: string;
  /** The card's note, for telling whether the server has rendered its latest change (see `PendingRender`). */
  noteId?: number;
  /** Opens a note linked from the card. */
  onOpenNote?: (noteId: number) => void;
  /** Changing it refetches the rendered file, e.g. after it was regenerated. */
  version?: number;
  /** Called once the card is displayed, or has failed to. */
  onReady?: () => void;
  /** Plays an audio clip linked from the card. Without it, the link is followed. */
  onPlayAudio?: (path: string) => void;
  /** Called with each width a Typst source is compiled for. */
  onTypstWidth?: (width: number) => void;
  /** Scrolls the card's clozes into view once it is displayed, e.g. while its side is shown. */
  scrollToCloze?: boolean;
}

export default function CardRenderer({
  path,
  source,
  webPath,
  deviceSource,
  noteId,
  onOpenNote,
  version,
  onReady,
  onPlayAudio,
  onTypstWidth,
  scrollToCloze,
}: Props) {
  // The card text size setting, wherever a card is shown
  const zoom = Number(useCardZoom()) / 100;
  // One page to a row on phones, whatever the setting
  const pagesPerRow = usePagesPerRow();
  const isNarrow = useMediaQuery("(max-width: 640px)");
  // Typst cards compile from the note's text here, so they are never behind
  const pending = usePendingRender(source === undefined ? noteId : undefined);
  if (source !== undefined)
    return (
      <TypstViewer
        source={source}
        onReady={onReady}
        onPlayAudio={onPlayAudio}
        onWidth={onTypstWidth}
        scrollToCloze={scrollToCloze}
        zoom={zoom}
        pagesPerRow={isNarrow ? "1" : pagesPerRow}
      />
    );
  const rendered =
    webPath !== undefined ? (
      <HtmlViewer
        path={webPath}
        version={version}
        onOpenNote={onOpenNote}
        onReady={onReady}
        onPlayAudio={onPlayAudio}
        scrollToCloze={scrollToCloze}
        zoom={zoom}
      />
    ) : (
      <RenderedFile path={path} version={version} onReady={onReady} />
    );
  if (!pending || noteId === undefined) return rendered;
  if (deviceSource !== undefined)
    return (
      <DeviceSource
        source={deviceSource}
        onOpenNote={onOpenNote}
        onReady={onReady}
        onPlayAudio={onPlayAudio}
        scrollToCloze={scrollToCloze}
        zoom={zoom}
      />
    );
  return (
    <PendingRender noteId={noteId} path={webPath ?? path} onReady={onReady}>
      {rendered}
    </PendingRender>
  );
}

function RenderedFile({
  path,
  version,
  onReady,
}: Pick<Props, "path" | "version" | "onReady">) {
  const { url, error } = useAuthedFileUrl(path, version);

  useEffect(() => {
    if (error !== null) onReady?.();
  }, [error, onReady]);

  if (error)
    return <div className="error-text">Error loading card: {error}</div>;
  if (url === null) return <div className="loading-delayed">Loading…</div>;
  return <PdfViewer url={url} onReady={onReady} />;
}

const badge: React.CSSProperties = {
  fontSize: 12,
  color: "var(--warning)",
  margin: "0 0 6px",
};

/**
 * A Markdown card of a note changed on this device, from its own source, so that it is never
 * behind: at once as `marked` shows it (see `markdownCardHtml`), then as pandoc converts it on the
 * device like the server does (see src/local/pandoc.ts), when it can. Its image occlusions are the
 * images the device makes.
 */
function DeviceSource({
  source,
  onOpenNote,
  onReady,
  onPlayAudio,
  scrollToCloze,
  zoom,
}: {
  source: string;
  onOpenNote?: (noteId: number) => void;
  onReady?: () => void;
  onPlayAudio?: (path: string) => void;
  scrollToCloze?: boolean;
  zoom: number;
}) {
  const parsed = useMemo(() => markdownCardHtml(source), [source]);
  const marked = useDeviceImages(parsed);
  const html = useDeviceImages(usePandocHtml(source)) ?? marked;
  return (
    <>
      <p style={badge}>
        Changed on this device: shown from its text until the server renders it,
        after the next sync.
      </p>
      {html === null ? (
        <div className="loading-delayed">Loading…</div>
      ) : (
        <HtmlViewer
          path=""
          html={html}
          onOpenNote={onOpenNote}
          onReady={onReady}
          onPlayAudio={onPlayAudio}
          scrollToCloze={scrollToCloze}
          zoom={zoom}
        />
      )}
    </>
  );
}

/** `html` with the images on this device shown (see `showDeviceImages`), or `null` until then. */
function useDeviceImages(html: string | null): string | null {
  const [withImages, setWithImages] = useState<{
    html: string;
    shown: string;
  } | null>(null);
  useEffect(() => {
    if (html === null) return;
    let cancelled = false;
    const urls: string[] = [];
    showDeviceImages(html, urls).then(
      (shown) => {
        if (!cancelled) setWithImages({ html, shown });
      },
      () => {
        if (!cancelled) setWithImages({ html, shown: html });
      },
    );
    return () => {
      cancelled = true;
      urls.forEach((url) => URL.revokeObjectURL(url));
    };
  }, [html]);
  return withImages !== null && withImages.html === html
    ? withImages.shown
    : null;
}

type Shown = { path: string } & (
  { rendered: true } | { note: NoteResponse } | { error: string }
);

/**
 * A card of a note changed on this device, which the server renders after the next sync, unless
 * it's a Markdown card (`DeviceSource`): as it was last rendered, or, for a note never rendered, as
 * the note's text (Markdown as HTML, sandboxed
 * like a rendered card, since a note's raw HTML could run scripts in the app otherwise).
 */
function PendingRender({
  noteId,
  path,
  onReady,
  children,
}: {
  noteId: number;
  path: string;
  onReady?: () => void;
  children: ReactNode;
}) {
  const [shown, setShown] = useState<Shown | null>(null);

  useEffect(() => {
    let cancelled = false;
    const show = (next: Shown) => {
      if (!cancelled) setShown(next);
    };
    fetchFile(path).then(
      () => show({ path, rendered: true }),
      () =>
        getNote(noteId).then(
          (note) => show({ path, note }),
          (e) => show({ path, error: errorMessage(e) }),
        ),
    );
    return () => {
      cancelled = true;
    };
  }, [noteId, path]);

  const current = shown?.path === path ? shown : null;
  const showsText = current !== null && !("rendered" in current);
  useEffect(() => {
    if (showsText) onReady?.();
  }, [showsText, onReady]);
  const markdown =
    current && "note" in current && current.note.parser_name === "markdown"
      ? current.note.data
      : null;
  const parsed = useMemo(
    () => (markdown === null ? null : marked.parse(markdown, { async: false })),
    [markdown],
  );
  const html = useDeviceImages(parsed) ?? parsed;

  if (current === null) return <div className="loading-delayed">Loading…</div>;
  if ("rendered" in current) {
    return (
      <>
        <p style={badge}>
          Changed on this device: shown as last rendered. The server renders it
          again after the next sync.
        </p>
        {children}
      </>
    );
  }
  if ("error" in current)
    return (
      <div className="error-text">Error loading card: {current.error}</div>
    );
  return (
    <>
      <p style={badge}>
        Not rendered yet: the server renders it after the next sync. Until then,
        this is the note&apos;s text.
      </p>
      {html !== null ? (
        <HtmlViewer path={path} html={html} />
      ) : (
        <pre style={{ whiteSpace: "pre-wrap", margin: 0 }}>
          {current.note.data}
        </pre>
      )}
    </>
  );
}

/**
 * `html` with the images it names by path that were added on this device shown from there, since
 * the server, where the note names them, doesn't have them until the next sync. Their object URLs
 * are added to `urls`, to revoke.
 */
async function showDeviceImages(html: string, urls: string[]): Promise<string> {
  const doc = new DOMParser().parseFromString(html, "text/html");
  const images = [...doc.images].filter((image) =>
    image.getAttribute("src")?.startsWith("/"),
  );
  if (images.length === 0) return html;
  // pandoc's, with its styles in the head
  const whole = /^\s*<!DOCTYPE/i.test(html);
  await Promise.all(
    images.map(async (image) => {
      const path = decodeURI(image.getAttribute("src")!);
      const type = imageMimeType(path);
      const data =
        type === null ? null : await deviceMediaFile(path).catch(() => null);
      if (!data) return;
      const url = URL.createObjectURL(
        new Blob([data as BlobPart], { type: type! }),
      );
      urls.push(url);
      image.src = url;
    }),
  );
  return whole
    ? `<!DOCTYPE html>\n${doc.documentElement.outerHTML}`
    : doc.body.innerHTML;
}
