import { useRef } from 'react';
import type { EditorView } from '@uiw/react-codemirror';
import { insertImageFiles } from '../imageUpload';

/**
 * Picks images to insert at the editor's cursor, as pasting or dropping them does. On a phone,
 * the picker offers the photo library and the camera, where there is nothing to paste or drop.
 */
export default function InsertImageButton({ parserId, view }: { parserId: number | null; view: () => EditorView | undefined }) {
  const input = useRef<HTMLInputElement>(null);
  return (
    <>
      <button
        onClick={() => input.current?.click()}
        disabled={parserId === null}
        className="button-tool tool-teal"
        title={parserId === null ? 'Select a parser first' : 'Choose images or take a photo, and insert them at the cursor'}
        style={{ padding: '2px 8px', fontSize: 12 }}
      >
        <span aria-hidden="true">📷</span> Insert image
      </button>
      <input
        ref={input}
        type="file"
        accept="image/*"
        multiple
        hidden
        onChange={event => {
          const files = Array.from(event.target.files ?? []);
          // So picking the same image again fires `change` again
          event.target.value = '';
          const editor = view();
          if (parserId !== null && editor && files.length > 0) insertImageFiles(editor, parserId, files);
        }}
      />
    </>
  );
}
