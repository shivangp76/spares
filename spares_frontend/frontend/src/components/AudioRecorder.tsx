import { useEffect, useRef, useState } from 'react';
import { uploadAudio } from '../api/client';
import { canRecordAudio, recordingFile, recordingMimeType } from '../audioRecording';

type State = 'idle' | 'starting' | 'recording' | 'recorded' | 'uploading';

function formatElapsed(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
}

/** Records an audio clip from the microphone, and hands over the link to the stored clip to insert
    into a note. */
export default function AudioRecorder({ parserId, onInserted, onClose }: {
  parserId: number;
  onInserted: (snippet: string) => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDialogElement>(null);
  const recorderRef = useRef<MediaRecorder | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const [state, setState] = useState<State>('idle');
  const [recording, setRecording] = useState<Blob | null>(null);
  const [recordingUrl, setRecordingUrl] = useState<string | null>(null);
  const [startedAt, setStartedAt] = useState(0);
  const [elapsed, setElapsed] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const supported = canRecordAudio();

  useEffect(() => {
    // Not closed on unmount, since its close event would fire `onClose` after StrictMode remounts it
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
  }, []);

  function stopStream() {
    streamRef.current?.getTracks().forEach(track => track.stop());
    streamRef.current = null;
  }

  // Let go of the microphone if closed while recording
  useEffect(() => () => {
    const recorder = recorderRef.current;
    if (recorder) recorder.ondataavailable = recorder.onstop = null;
    if (recorder?.state === 'recording') recorder.stop();
    stopStream();
  }, []);

  useEffect(() => () => { if (recordingUrl) URL.revokeObjectURL(recordingUrl); }, [recordingUrl]);

  useEffect(() => {
    if (state !== 'recording') return;
    const timer = setInterval(() => setElapsed(Date.now() - startedAt), 200);
    return () => clearInterval(timer);
  }, [state, startedAt]);

  async function start() {
    setError(null);
    setState('starting');
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (e) {
      setError(e instanceof DOMException && e.name === 'NotAllowedError'
        ? 'Microphone access was denied. Allow it in the browser\'s site settings.'
        : `Could not use the microphone: ${String(e)}`);
      setState(recording ? 'recorded' : 'idle');
      return;
    }
    streamRef.current = stream;
    const mimeType = recordingMimeType();
    const recorder = new MediaRecorder(stream, mimeType ? { mimeType } : undefined);
    const chunks: Blob[] = [];
    recorder.ondataavailable = e => { if (e.data.size > 0) chunks.push(e.data); };
    recorder.onstop = () => {
      stopStream();
      const blob = new Blob(chunks, { type: recorder.mimeType });
      setRecording(blob);
      setRecordingUrl(URL.createObjectURL(blob));
      setState('recorded');
    };
    recorderRef.current = recorder;
    recorder.start();
    setStartedAt(Date.now());
    setElapsed(0);
    setState('recording');
  }

  function stop() {
    recorderRef.current?.stop();
  }

  async function insert() {
    if (!recording) return;
    setError(null);
    setState('uploading');
    try {
      const { snippet } = await uploadAudio(parserId, await recordingFile(recording));
      onInserted(snippet);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
      setState('recorded');
    }
  }

  /** Asks before throwing a recording away. */
  function confirmDiscard(): boolean {
    return (state !== 'recording' && !recording) || window.confirm('Discard the recording?');
  }

  return (
    <dialog
      ref={dialogRef}
      className="audio-recorder"
      aria-labelledby="audio-recorder-title"
      onClose={onClose}
      onCancel={e => { if (state === 'uploading' || !confirmDiscard()) e.preventDefault(); }}
    >
      <div className="audio-recorder-content">
        <h3 id="audio-recorder-title" style={{ margin: '0 0 12px' }}>Record audio</h3>
        {!supported ? (
          <p style={{ fontSize: 13, color: 'var(--text-secondary)' }}>
            This browser can't record audio here. Microphones are only available on pages served over HTTPS or from localhost.
          </p>
        ) : (
          <>
            <div style={{ display: 'flex', alignItems: 'center', gap: 12, minHeight: 36 }}>
              {state === 'recording' ? (
                <button onClick={stop} autoFocus className="touch-target-small" style={{ padding: '5px 14px', fontSize: 13 }}>
                  Stop
                </button>
              ) : (
                <button
                  onClick={() => { if (!recording || confirmDiscard()) void start(); }}
                  disabled={state === 'starting' || state === 'uploading'}
                  autoFocus
                  className="touch-target-small"
                  style={{ padding: '5px 14px', fontSize: 13 }}
                >
                  {recording ? 'Record again' : 'Record'}
                </button>
              )}
              {state === 'recording' && (
                <span style={{ display: 'flex', alignItems: 'center', gap: 6, fontSize: 13, fontVariantNumeric: 'tabular-nums' }}>
                  <span className="audio-recorder-dot" aria-hidden="true" />
                  Recording {formatElapsed(elapsed)}
                </span>
              )}
              {state === 'starting' && <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>Waiting for the microphone…</span>}
              {state === 'idle' && !error && <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>The clip is inserted as a link at the cursor.</span>}
            </div>
            {recordingUrl && state !== 'recording' && (
              <audio src={recordingUrl} controls style={{ width: '100%', marginTop: 12 }} />
            )}
          </>
        )}
        {error && <p style={{ fontSize: 13, color: 'var(--error)', margin: '12px 0 0' }}>{error}</p>}
        <div style={{ display: 'flex', justifyContent: 'flex-end', gap: 8, marginTop: 16 }}>
          <button
            onClick={() => { if (confirmDiscard()) dialogRef.current?.close(); }}
            disabled={state === 'uploading'}
            className="touch-target-small"
            style={{ padding: '5px 14px', fontSize: 13 }}
          >
            Cancel
          </button>
          <button
            onClick={() => void insert()}
            disabled={state !== 'recorded'}
            className="touch-target-small"
            style={{ padding: '5px 14px', fontSize: 13 }}
          >
            {state === 'uploading' ? 'Inserting…' : 'Insert'}
          </button>
        </div>
      </div>
    </dialog>
  );
}
