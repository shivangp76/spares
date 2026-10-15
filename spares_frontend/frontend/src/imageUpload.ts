import { Decoration, type DecorationSet, EditorView, type Extension, StateEffect, StateField, WidgetType } from '@uiw/react-codemirror';
import { uploadImage } from './api/client';
import { imageUploadName } from './utils';

class UploadingWidget extends WidgetType {
  constructor(readonly count: number) { super(); }

  eq(other: UploadingWidget) { return other.count === this.count; }

  toDOM() {
    const span = document.createElement('span');
    span.textContent = this.count === 1 ? 'Uploading image…' : `Uploading ${this.count} images…`;
    span.style.cssText = 'color: var(--text-muted); font-style: italic; padding: 0 4px;';
    return span;
  }
}

const addUpload = StateEffect.define<{ id: number; pos: number; count: number }>();
const removeUpload = StateEffect.define<number>();

/** Where each upload's images will be inserted, shown as a placeholder. As decorations, the
    places follow the edits made while the images upload. */
const uploads = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(set, tr) {
    set = set.map(tr.changes);
    for (const effect of tr.effects) {
      if (effect.is(addUpload)) {
        const { id, pos, count } = effect.value;
        // Before text typed at the same place, since the images came first
        const widget = Decoration.widget({ widget: new UploadingWidget(count), side: -1, id });
        set = set.update({ add: [widget.range(pos)] });
      } else if (effect.is(removeUpload)) {
        set = set.update({ filter: (_from, _to, value) => value.spec.id !== effect.value });
      }
    }
    return set;
  },
  provide: field => EditorView.decorations.from(field),
});

function uploadPos(view: EditorView, id: number): number | null {
  let pos: number | null = null;
  view.state.field(uploads).between(0, view.state.doc.length, (from, _to, value) => {
    if (value.spec.id === id) pos = from;
  });
  return pos;
}

let nextUploadId = 0;

/** Uploads `files` and inserts the snippets showing them at `pos`. Files whose upload failed are
    reported, and the rest are still inserted. */
async function insertImages(view: EditorView, parserId: number, files: File[], pos: number) {
  const id = nextUploadId++;
  view.dispatch({ effects: addUpload.of({ id, pos, count: files.length }) });
  const results = await Promise.allSettled(files.map(file => {
    const name = imageUploadName(file);
    if (name === null) return Promise.reject(new Error(`${file.name || 'The image'} is not a supported image format`));
    // Upload under the name with a supported extension
    return uploadImage(parserId, name === file.name ? file : new File([file], name, { type: file.type }));
  }));
  const snippets = results.flatMap(r => (r.status === 'fulfilled' ? [r.value.snippet] : []));
  const errors = results.flatMap(r => (r.status === 'rejected' ? [String(r.reason)] : []));

  // The editor may have been closed while uploading
  if (!view.dom.isConnected) return;
  const at = uploadPos(view, id);
  if (at === null) return;
  const insert = snippets.join('\n');
  const { main } = view.state.selection;
  // Follow the images with the cursor if it is still where they were added
  const selection = main.empty && main.head === at ? { anchor: at + insert.length } : undefined;
  view.dispatch({ changes: { from: at, insert }, selection, effects: removeUpload.of(id) });
  if (errors.length > 0) window.alert(errors.join('\n'));
}

function imageFiles(data: DataTransfer | null): File[] {
  return Array.from(data?.files ?? []).filter(file => file.type.startsWith('image/'));
}

/** Uploads images pasted or dropped into the editor, and inserts them in the syntax of the note's
    parser where they were pasted or dropped. */
export function imageUpload(parserId: number): Extension {
  return [
    uploads,
    EditorView.domEventHandlers({
      paste(event, view) {
        // Spreadsheets also put a picture of copied cells on the clipboard, but the text is wanted
        if (event.clipboardData?.getData('text/plain')) return false;
        const files = imageFiles(event.clipboardData);
        if (files.length === 0) return false;
        event.preventDefault();
        void insertImages(view, parserId, files, view.state.selection.main.head);
        return true;
      },
      drop(event, view) {
        const files = imageFiles(event.dataTransfer);
        if (files.length === 0) return false;
        // Otherwise CodeMirror inserts the files' contents as text
        event.preventDefault();
        const pos = view.posAtCoords({ x: event.clientX, y: event.clientY }) ?? view.state.selection.main.head;
        void insertImages(view, parserId, files, pos);
        return true;
      },
    }),
  ];
}
