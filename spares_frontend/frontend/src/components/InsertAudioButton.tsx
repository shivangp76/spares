import { useState } from 'react';
import AudioRecorder from './AudioRecorder';

/** Opens the audio recorder, and hands over the link to the stored clip to insert into a note. */
export default function InsertAudioButton({ parserId, onInsert }: { parserId: number | null; onInsert: (snippet: string) => void }) {
  const [open, setOpen] = useState(false);
  return (
    <>
      <button
        onClick={() => setOpen(true)}
        disabled={parserId === null}
        className="button-tool"
        title={parserId === null ? 'Select a parser first' : 'Record an audio clip and insert it at the cursor'}
        style={{ padding: '2px 8px', fontSize: 12 }}
      >
        Record audio
      </button>
      {open && parserId !== null && (
        <AudioRecorder
          parserId={parserId}
          onInserted={snippet => { setOpen(false); onInsert(snippet); }}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}
