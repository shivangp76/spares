import { useState } from "react";
import { OCCLUSIONS_NEED_SERVER, useLocalStatus } from "../local/copy";
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
  const local = useLocalStatus();
  let title = "Draw clozes over an image and insert them at the cursor";
  if (local.active) title = OCCLUSIONS_NEED_SERVER;
  else if (parserId === null) title = "Select a parser first";
  return (
    <>
      <button
        onClick={() => setOpen(true)}
        disabled={parserId === null || local.active}
        className="button-tool tool-purple"
        title={title}
        style={{ padding: "2px 8px", fontSize: 12 }}
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
