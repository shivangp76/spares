import { Decoration, type DecorationSet, EditorView, type Extension, StateEffect, StateField, ViewPlugin, WidgetType } from '@uiw/react-codemirror';
import { uploadImage, uploadImageUrl } from './api/client';
import { draggedImageUrl, imageUploadName } from './utils';

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

/** Stores an image and returns the snippet showing it. */
type ImageUpload = () => Promise<string>;

function fileUpload(parserId: number, file: File): ImageUpload {
  return async () => {
    const name = imageUploadName(file);
    if (name === null) throw new Error(`${file.name || 'The image'} is not a supported image format`);
    // Upload under the name with a supported extension
    const { snippet } = await uploadImage(parserId, name === file.name ? file : new File([file], name, { type: file.type }));
    return snippet;
  };
}

/** An image dragged from a web page, by its URL. */
function urlUpload(parserId: number, url: string): ImageUpload {
  return async () => {
    if (!url.startsWith('data:')) return (await uploadImageUrl(parserId, url)).snippet;
    const blob = await (await fetch(url)).blob();
    return fileUpload(parserId, new File([blob], '', { type: blob.type }))();
  };
}

let nextUploadId = 0;

/** Stores images and inserts the snippets showing them at `pos`. Images that failed to store are
    reported, and the rest are still inserted. */
async function insertImages(view: EditorView, images: ImageUpload[], pos: number) {
  const id = nextUploadId++;
  view.dispatch({ effects: addUpload.of({ id, pos, count: images.length }) });
  const results = await Promise.allSettled(images.map(upload => upload()));
  const snippets = results.flatMap(r => (r.status === 'fulfilled' ? [r.value] : []));
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

/** The images in dropped or pasted data: its image files, or else an image dragged from a web
    page, which Firefox hands over as HTML, not as a file. */
function imageUploads(parserId: number, data: DataTransfer | null): ImageUpload[] {
  const files = imageFiles(data);
  if (files.length > 0) return files.map(file => fileUpload(parserId, file));
  const url = draggedImageUrl(data?.getData('text/html') ?? '');
  return url === null ? [] : [urlUpload(parserId, url)];
}

/** Types Firefox gives a drag of an image from a web page, which has no files. */
const FIREFOX_IMAGE_DRAG_TYPES = ['application/x-moz-nativeimage', 'application/x-moz-file-promise-url'];

/** Whether a drag carries images. Only the files' types are known until they are dropped, and
    some browsers hide those too, so a drag of files of unknown types might be images. */
function dragsImages(data: DataTransfer | null): boolean {
  if (!data) return false;
  if (data.types.some(type => FIREFOX_IMAGE_DRAG_TYPES.includes(type))) return true;
  if (!data.types.includes('Files')) return false;
  const files = Array.from(data.items).filter(item => item.kind === 'file');
  return files.length === 0 || files.some(item => item.type === '' || item.type.startsWith('image/'));
}

/** How long after the last `dragover` a drag is taken to have ended, in case no event says so.
    Browsers repeat `dragover` every 350 ms or so while dragging over the page. */
const DRAG_END_DELAY_MS = 1000;

/** Accepts images dropped anywhere on the editor, gutters and padding included, not only on its
    text. While images are dragged over the page, a banner over the editor shows they can be
    dropped there. */
const imageDrop = (parserId: number) => ViewPlugin.define(view => {
  const banner = document.createElement('div');
  banner.className = 'cm-image-drop-banner';
  banner.setAttribute('aria-hidden', 'true');
  const label = document.createElement('span');
  banner.append(label);
  let hideTimer: ReturnType<typeof setTimeout> | undefined;

  function show(over: boolean) {
    label.textContent = over ? 'Release to insert' : 'Drop images here to insert them';
    banner.classList.toggle('cm-image-drop-banner-over', over);
    if (!banner.isConnected) view.dom.append(banner);
    clearTimeout(hideTimer);
    hideTimer = setTimeout(hide, DRAG_END_DELAY_MS);
  }
  function hide() {
    clearTimeout(hideTimer);
    banner.remove();
  }

  const onWindowDrag = (event: DragEvent) => {
    if (!dragsImages(event.dataTransfer)) return;
    // Over the editor, `onEditorDragOver` shows the banner
    if (!view.dom.contains(event.target as Node)) show(false);
  };
  // Leaving the window, or cancelling the drag, which some browsers report at (0, 0). Not
  // `relatedTarget`, which Safari leaves out between elements too.
  const onWindowDragLeave = (event: DragEvent) => {
    const { clientX: x, clientY: y } = event;
    if (x <= 0 || y <= 0 || x >= window.innerWidth || y >= window.innerHeight) hide();
  };
  // Captured so they run before CodeMirror's handlers on the text, which would insert a dropped
  // file's contents as text. CodeMirror skips its handlers for events already handled.
  const onEditorDragOver = (event: DragEvent) => {
    if (!dragsImages(event.dataTransfer)) return;
    // Allow the drop anywhere on the editor
    event.preventDefault();
    if (event.dataTransfer) event.dataTransfer.dropEffect = 'copy';
    show(true);
  };
  const onEditorDrop = (event: DragEvent) => {
    hide();
    const images = imageUploads(parserId, event.dataTransfer);
    if (images.length === 0) return;
    event.preventDefault();
    // Not precise, so a drop beside or below the text goes to its nearest place
    const pos = view.posAtCoords({ x: event.clientX, y: event.clientY }, false);
    view.focus();
    void insertImages(view, images, pos);
  };

  window.addEventListener('dragenter', onWindowDrag);
  window.addEventListener('dragover', onWindowDrag);
  window.addEventListener('dragleave', onWindowDragLeave);
  window.addEventListener('drop', hide);
  window.addEventListener('dragend', hide);
  view.dom.addEventListener('dragover', onEditorDragOver, true);
  view.dom.addEventListener('drop', onEditorDrop, true);
  return {
    destroy() {
      hide();
      window.removeEventListener('dragenter', onWindowDrag);
      window.removeEventListener('dragover', onWindowDrag);
      window.removeEventListener('dragleave', onWindowDragLeave);
      window.removeEventListener('drop', hide);
      window.removeEventListener('dragend', hide);
      view.dom.removeEventListener('dragover', onEditorDragOver, true);
      view.dom.removeEventListener('drop', onEditorDrop, true);
    },
  };
});

const imageDropTheme = EditorView.baseTheme({
  '.cm-image-drop-banner': {
    position: 'absolute',
    inset: '0',
    // Above the gutters
    zIndex: '300',
    display: 'flex',
    alignItems: 'center',
    justifyContent: 'center',
    border: '2px dashed var(--accent)',
    borderRadius: '4px',
    background: 'color-mix(in srgb, var(--accent) 8%, transparent)',
    // Drag events go to the editor underneath, so the drop lands there
    pointerEvents: 'none',
  },
  '.cm-image-drop-banner-over': {
    background: 'color-mix(in srgb, var(--accent) 18%, transparent)',
  },
  '.cm-image-drop-banner span': {
    padding: '4px 10px',
    borderRadius: '4px',
    background: 'var(--bg)',
    color: 'var(--accent)',
    fontSize: '13px',
    fontWeight: '600',
  },
});

/** Uploads images pasted or dropped into the editor, and inserts them in the syntax of the note's
    parser where they were pasted or dropped. */
export function imageUpload(parserId: number): Extension {
  return [
    uploads,
    imageDrop(parserId),
    imageDropTheme,
    EditorView.domEventHandlers({
      paste(event, view) {
        // Spreadsheets also put a picture of copied cells on the clipboard, but the text is wanted
        if (event.clipboardData?.getData('text/plain')) return false;
        const images = imageFiles(event.clipboardData).map(file => fileUpload(parserId, file));
        if (images.length === 0) return false;
        event.preventDefault();
        void insertImages(view, images, view.state.selection.main.head);
        return true;
      },
    }),
  ];
}
