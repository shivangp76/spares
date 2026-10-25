import { useEffect, useMemo, useState, type ReactNode } from "react";
import { marked } from "marked";
import { fetchFile, getNote } from "../api/client";
import { useAuthedFileUrl } from "../hooks/useAuthedFileUrl";
import { usePendingRender } from "../local/copy";
import type { NoteResponse } from "../types/spares";
import { errorMessage } from "../utils";
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
    return (
      <div style={{ color: "var(--error)" }}>Error loading card: {error}</div>
    );
  if (url === null) return <div className="loading-delayed">Loading…</div>;
  return <PdfViewer url={url} onReady={onReady} />;
}

const badge: React.CSSProperties = {
  fontSize: 12,
  color: "var(--warning)",
  margin: "0 0 6px",
};

type Shown = { path: string } & (
  { rendered: true } | { note: NoteResponse } | { error: string }
);

/**
 * A card of a note changed on this device, which the server renders after the next sync: as it
 * was last rendered, or, for a note never rendered, as the note's text (Markdown as HTML, sandboxed
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
  const html = useMemo(
    () => (markdown === null ? null : marked.parse(markdown, { async: false })),
    [markdown],
  );

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
      <div style={{ color: "var(--error)" }}>
        Error loading card: {current.error}
      </div>
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
