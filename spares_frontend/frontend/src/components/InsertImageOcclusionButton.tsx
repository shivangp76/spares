import { useState } from 'react';
import { useMediaQuery } from '../hooks/useMediaQuery';
import ImageOcclusionEditor from './ImageOcclusionEditor';

/** Opens the image occlusion editor, and hands over the stored image occlusion's block to insert into a note. */
export default function InsertImageOcclusionButton({ parserId, onInsert }: { parserId: number | null; onInsert: (snippet: string) => void }) {
  const [open, setOpen] = useState(false);
  // svgedit needs a pointer and room for its panels
  const isNarrow = useMediaQuery('(max-width: 640px)');
  if (isNarrow) return null;
  return (
    <>
      <button
        onClick={() => setOpen(true)}
        disabled={parserId === null}
        title={parserId === null ? 'Select a parser first' : 'Draw clozes over an image and insert them at the cursor'}
        style={{ padding: '2px 8px', fontSize: 12 }}
      >
        Insert image occlusion
      </button>
      {open && parserId !== null && (
        <ImageOcclusionEditor
          mode="new"
          parserId={parserId}
          onCreated={snippet => { setOpen(false); onInsert(snippet); }}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}
