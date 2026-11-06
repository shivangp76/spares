import { useState } from "react";
import ImageOcclusionEditor from "./ImageOcclusionEditor";

/** Opens the image occlusion editor, and hands over the stored image occlusion's block to insert into a note. */
export default function InsertImageOcclusionButton({
  parserId,
  onInsert,
}: {
  parserId: number | null;
  onInsert: (snippet: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const title =
    parserId === null
      ? "Select a parser first"
      : "Draw clozes over an image and insert them at the cursor";
  return (
    <>
      <button
        onClick={() => setOpen(true)}
        disabled={parserId === null}
        className="button-tool tool-purple button-small"
        title={title}
      >
        <span aria-hidden="true">🖼️</span> Insert image occlusion
      </button>
      {open && parserId !== null && (
        <ImageOcclusionEditor
          mode="new"
          parserId={parserId}
          onCreated={(snippet) => {
            setOpen(false);
            onInsert(snippet);
          }}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}
