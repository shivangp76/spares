import { clipName } from '../audio';
import type { AudioPlayer } from '../hooks/useAudioPlayer';
import type { CardAudio } from '../types/spares';

const button: React.CSSProperties = { fontSize: 13, padding: '4px 10px' };

/** A button per clip of the side shown, and Replay, which plays the front's clips and then the back's. */
export default function AudioBar({ audio, flipped, player }: { audio: CardAudio; flipped: boolean; player: AudioPlayer }) {
  const clips = flipped ? [...audio.front, ...audio.back] : audio.front;
  if (clips.length === 0 && !player.error) return null;
  return (
    <div className="audio-bar" style={{ display: 'flex', gap: 8, flexWrap: 'wrap', alignItems: 'center', marginBottom: 12 }}>
      {player.blocked && (
        <button onClick={() => player.play(player.blocked!)} style={{ ...button, fontWeight: 600 }} title="The browser blocked playing audio before the page was used">
          ▶ Play
        </button>
      )}
      {clips.map(path => (
        <button
          key={path}
          onClick={() => player.play([path])}
          style={{ ...button, fontWeight: player.playing === path ? 600 : undefined }}
          title={path}
        >
          {player.playing === path ? '🔊' : '🔈'} {clipName(path)}
        </button>
      ))}
      {clips.length > 0 && (
        <button onClick={() => player.play(clips)} style={button}>
          Replay <span className="key-hint">(r)</span>
        </button>
      )}
      {player.error && <span style={{ fontSize: 13, color: 'var(--error)' }}>{player.error}</span>}
    </div>
  );
}
