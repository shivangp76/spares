import { useEffect, useId, useRef, useState } from "react";
import { fetchUnmaskedImageOcclusion } from "../api/client";
import type { UnmaskedImageOcclusion } from "../types/spares";

/** An image occlusion's parts, as object URLs. */
interface Parts {
  image: string;
  markup: string | null;
}

/**
 * A popup with `occlusions`, of card `cardOrder` of note `noteId`, without their masks: each
 * original image with its markup over it at the same width, captioned like the card's figure.
 * Fetched when it opens, and closed by Esc, the Close button or a click on the backdrop.
 */
export default function UnmaskedImageOcclusions({
  noteId,
  cardOrder,
  occlusions,
  onClose,
}: {
  noteId: number;
  cardOrder: number;
  occlusions: UnmaskedImageOcclusion[];
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const [parts, setParts] = useState<Record<number, Parts>>({});
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  useEffect(() => {
    let closed = false;
    const urls: string[] = [];
    const revoke = (p: Parts) =>
      [p.image, p.markup].forEach((url) => url && URL.revokeObjectURL(url));
    for (const { order } of occlusions) {
      fetchUnmaskedImageOcclusion(noteId, cardOrder, order)
        .then((fetched) => {
          if (closed) {
            revoke(fetched);
            return;
          }
          urls.push(fetched.image, ...(fetched.markup ? [fetched.markup] : []));
          setParts((shown) => ({ ...shown, [order]: fetched }));
        })
        .catch((e) => {
          if (!closed) setError(e instanceof Error ? e.message : String(e));
        });
    }
    return () => {
      closed = true;
      urls.forEach((url) => URL.revokeObjectURL(url));
    };
  }, [noteId, cardOrder, occlusions]);

  return (
    <dialog
      ref={dialogRef}
      className="help-dialog unmasked-image-occlusions"
      aria-labelledby={titleId}
      onClose={onClose}
      // The dialog itself is only hit by a click on the backdrop, as its content fills it
      onClick={(e) => {
        if (e.target === e.currentTarget) dialogRef.current?.close();
      }}
    >
      <div className="help-dialog-content">
        <div
          style={{
            display: "flex",
            alignItems: "baseline",
            justifyContent: "space-between",
            gap: 16,
          }}
        >
          <h3 id={titleId} style={{ margin: 0 }}>
            Without masks
          </h3>
          <button type="button" onClick={() => dialogRef.current?.close()}>
            Close <span className="key-hint">(m, Esc)</span>
          </button>
        </div>
        {error && (
          <div style={{ marginTop: 12, color: "var(--danger)", fontSize: 13 }}>
            {error}
          </div>
        )}
        {occlusions.map(({ order, caption }) => {
          const shown = parts[order];
          return (
            <figure
              key={order}
              style={{ margin: "12px 0 0", textAlign: "center" }}
            >
              {shown ? (
                // The markup is the original image's size, so it lines up at the same width
                <div
                  style={{
                    position: "relative",
                    display: "inline-block",
                    maxWidth: "100%",
                    verticalAlign: "top",
                  }}
                >
                  <img
                    src={shown.image}
                    alt={caption}
                    style={{
                      display: "block",
                      maxWidth: "100%",
                      maxHeight: "75vh",
                    }}
                  />
                  {shown.markup && (
                    <img
                      src={shown.markup}
                      alt=""
                      aria-hidden="true"
                      style={{
                        position: "absolute",
                        inset: 0,
                        width: "100%",
                        height: "100%",
                      }}
                    />
                  )}
                </div>
              ) : (
                !error && <div className="hint">Loading…</div>
              )}
              <figcaption
                style={{
                  fontSize: 13,
                  color: "var(--text-secondary)",
                  marginTop: 4,
                }}
              >
                {caption}
              </figcaption>
            </figure>
          );
        })}
      </div>
    </dialog>
  );
}
