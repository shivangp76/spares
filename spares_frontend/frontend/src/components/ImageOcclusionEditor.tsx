import { useEffect, useRef, useState } from 'react';
import { createImageOcclusion, fetchImageOcclusionFile, getImageOcclusionTemplate } from '../api/client';
import type { ImageOcclusionData } from '../types/spares';

/** Set on the editor's window by the `ext-spares` svgedit extension when it is embedded. */
interface SparesBridge {
  ready: Promise<void>;
  load(args: { imageUrl: string; width: number; height: number; svg: string; title?: string }): Promise<void>;
  getClozesSvg(): string;
  isDirty(): boolean;
}

type Props =
  | {
    mode: 'new';
    parserId: number;
    /** Given the image occlusion block to insert into a note, once it is stored. */
    onCreated: (snippet: string) => void;
    onClose: () => void;
  }
  | {
    mode: 'edit';
    occlusion: ImageOcclusionData;
    /** Stores the edited clozes. The editor stays open with the error if this throws. */
    onSave: (clozesSvg: string) => Promise<void>;
    onClose: () => void;
  };

const EDITOR_PATH = '/svgedit/src/editor/index.html';

function imageSize(url: string): Promise<{ width: number; height: number }> {
  return new Promise((resolve, reject) => {
    const img = new Image();
    img.onload = () => resolve({ width: img.naturalWidth, height: img.naturalHeight });
    img.onerror = () => reject(new Error('The image could not be read'));
    img.src = url;
  });
}

function fileName(path: string): string {
  return path.split('/').pop() ?? path;
}

/** svgedit in a full-screen iframe, for drawing an image occlusion's clozes over its image. svgedit
    keeps its own colours whatever the app's theme, since its panels are not designed for a dark one. */
export default function ImageOcclusionEditor(props: Props) {
  const { mode, onClose } = props;
  const iframeRef = useRef<HTMLIFrameElement>(null);
  const [bridge, setBridge] = useState<SparesBridge | null>(null);
  const [image, setImage] = useState<File | null>(null);
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // svgedit shows the image by URL, so object URLs live until the editor closes
  const objectUrls = useRef<string[]>([]);

  useEffect(() => {
    const urls = objectUrls.current;
    const { overflow } = document.body.style;
    document.body.style.overflow = 'hidden';
    return () => {
      document.body.style.overflow = overflow;
      urls.forEach(url => URL.revokeObjectURL(url));
    };
  }, []);

  // The extension posts `spares:ready` once the bridge is set, which may be before React sees the
  // iframe load, so check for the bridge on both
  useEffect(() => {
    function connect() {
      const found = (iframeRef.current?.contentWindow as (Window & { sparesBridge?: SparesBridge }) | null)?.sparesBridge;
      if (found) found.ready.then(() => setBridge(found));
    }
    function onMessage(event: MessageEvent) {
      if (event.origin === window.location.origin && event.data?.type === 'spares:ready') connect();
    }
    window.addEventListener('message', onMessage);
    const iframe = iframeRef.current;
    iframe?.addEventListener('load', connect);
    connect();
    return () => {
      window.removeEventListener('message', onMessage);
      iframe?.removeEventListener('load', connect);
    };
  }, []);

  function objectUrl(blob: Blob): string {
    const url = URL.createObjectURL(blob);
    objectUrls.current.push(url);
    return url;
  }

  // Edit mode: load the stored image and clozes
  const occlusion = mode === 'edit' ? props.occlusion : null;
  useEffect(() => {
    if (!bridge || !occlusion) return;
    let cancelled = false;
    (async () => {
      const [imageBlob, svgBlob] = await Promise.all([
        fetchImageOcclusionFile(occlusion.original_image_filepath),
        fetchImageOcclusionFile(occlusion.clozes_filepath),
      ]);
      if (cancelled) return;
      const imageUrl = objectUrl(imageBlob);
      const { width, height } = await imageSize(imageUrl);
      await bridge.load({ imageUrl, width, height, svg: await svgBlob.text(), title: fileName(occlusion.clozes_filepath) });
      if (!cancelled) setLoaded(true);
    })().catch(e => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, [bridge, occlusion]);

  async function chooseImage(file: File) {
    if (!bridge) return;
    if (!file.type.startsWith('image/')) { setError(`${file.name} is not an image`); return; }
    if (loaded && bridge.isDirty() && !window.confirm('Replace the image? The clozes drawn so far are discarded.')) return;
    setError(null);
    try {
      const imageUrl = objectUrl(file);
      const { width, height } = await imageSize(imageUrl);
      await bridge.load({ imageUrl, width, height, svg: await getImageOcclusionTemplate(), title: file.name });
      setImage(file);
      setLoaded(true);
    } catch (e) {
      setError(String(e));
    }
  }

  // New mode: an image can also be pasted. svgedit has focus while drawing, so listen in the iframe too.
  useEffect(() => {
    if (mode !== 'new' || !bridge) return;
    const frameWindow = iframeRef.current?.contentWindow;
    function onPaste(event: ClipboardEvent) {
      const file = Array.from(event.clipboardData?.files ?? []).find(f => f.type.startsWith('image/'));
      if (!file) return;
      event.preventDefault();
      chooseImage(file);
    }
    window.addEventListener('paste', onPaste);
    frameWindow?.addEventListener('paste', onPaste);
    return () => {
      window.removeEventListener('paste', onPaste);
      frameWindow?.removeEventListener('paste', onPaste);
    };
  });

  async function save() {
    if (!bridge) return;
    setSaving(true);
    setError(null);
    try {
      const svg = bridge.getClozesSvg();
      if (props.mode === 'new') {
        if (!image) throw new Error('Choose an image first');
        const { snippet } = await createImageOcclusion(props.parserId, image, svg);
        props.onCreated(snippet);
      } else {
        await props.onSave(svg);
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  }

  function close() {
    if (loaded && bridge?.isDirty() && !window.confirm('Discard the changes to this image occlusion?')) return;
    onClose();
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Image occlusion editor"
      style={{ position: 'fixed', inset: 0, zIndex: 1000, display: 'flex', flexDirection: 'column', background: 'var(--bg)' }}
      onDragOver={e => { if (mode === 'new') e.preventDefault(); }}
      onDrop={e => {
        if (mode !== 'new') return;
        e.preventDefault();
        const file = e.dataTransfer.files[0];
        if (file) chooseImage(file);
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 10, padding: '8px 12px', borderBottom: '1px solid var(--border-subtle)', flexWrap: 'wrap' }}>
        <strong style={{ fontSize: 14 }}>{mode === 'new' ? 'New image occlusion' : `Edit ${fileName(props.occlusion.original_image_filepath)}`}</strong>
        {mode === 'new' && (
          <label style={{ fontSize: 13 }}>
            <input
              type="file"
              accept="image/*"
              disabled={!bridge}
              onChange={e => { const file = e.target.files?.[0]; if (file) chooseImage(file); e.target.value = ''; }}
            />
          </label>
        )}
        <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>
          {!bridge
            ? 'Loading editor…'
            : mode === 'new' && !image
              ? 'Choose, paste or drop an image, then draw clozes on the Clozes layer.'
              : !loaded
                ? 'Loading image occlusion…'
                : 'Shapes on the Clozes layer become clozes. Set a cloze\'s settings in its Cloze Settings field.'}
        </span>
        {error && <span style={{ fontSize: 13, color: 'var(--error)' }}>{error}</span>}
        <div style={{ marginLeft: 'auto', display: 'flex', gap: 8 }}>
          <button onClick={close} disabled={saving} className="touch-target-small" style={{ padding: '5px 14px', fontSize: 13 }}>Cancel</button>
          <button onClick={save} disabled={saving || !loaded} className="touch-target-small" style={{ padding: '5px 14px', fontSize: 13 }}>
            {saving ? 'Saving…' : mode === 'new' ? 'Save and insert' : 'Save'}
          </button>
        </div>
      </div>
      <iframe
        ref={iframeRef}
        src={`${EDITOR_PATH}?embedded=1&storagePrompt=false`}
        title="Image occlusion editor"
        style={{ flex: 1, border: 'none', width: '100%' }}
      />
    </div>
  );
}
