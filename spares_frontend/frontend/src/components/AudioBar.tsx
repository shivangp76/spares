import { audioItemKey, audioItemLabel, isSpeech } from '../audio';
import type { AudioPlayer } from '../hooks/useAudioPlayer';
import type { CardAudio } from '../types/spares';

const button: React.CSSProperties = { fontSize: 13, padding: '4px 10px' };

/** A button per clip or text to read aloud of the side shown, and Replay, which plays the front's
    and then the back's. */
export default function AudioBar({ audio, flipped, player }: { audio: CardAudio; flipped: boolean; player: AudioPlayer }) {
  const items = flipped ? [...audio.front, ...audio.back] : audio.front;
  if (items.length === 0 && !player.error) return null;
  return (
    <div className="audio-bar" style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center', marginBottom: 12 }}>
      {player.blocked && (
        <button onClick={() => player.play(player.blocked!)} style={{ ...button, fontWeight: 600 }} title="The browser blocked playing audio before the page was used">
          ▶ Play
        </button>
      )}
      {items.map(item => {
        const key = audioItemKey(item);
        const playing = player.playing === key;
        return (
          <button
            key={key}
            onClick={() => player.play([item])}
            style={{ ...button, fontWeight: playing ? 600 : undefined }}
            title={isSpeech(item) ? `Read aloud${item.lang ? ` (${item.lang})` : ''}: ${item.text}` : item}
          >
            {isSpeech(item) ? (playing ? '🗣️' : '💬') : (playing ? '🔊' : '🔈')} {audioItemLabel(item)}
          </button>
        );
      })}
      {items.length > 0 && (
        <button onClick={() => player.play(items)} style={button}>
          Replay <span className="key-hint">(r)</span>
        </button>
      )}
      {player.error && <span style={{ fontSize: 13, color: 'var(--error)' }}>{player.error}</span>}
    </div>
  );
}
