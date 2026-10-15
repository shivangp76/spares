//! Plays a card's audio clips with the configured `audio_player`, without waiting for them.

use std::path::Path;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use spares_core::parsers::audio::CardAudio;

/// Plays clips one after another in the background. Playing again, or dropping it, stops what
/// was playing.
pub(crate) struct AudioPlayer {
    command: String,
    current: Option<Playback>,
}

struct Playback {
    stopped: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
}

impl Playback {
    fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        if let Some(child) = self.child.lock().unwrap().as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl AudioPlayer {
    /// `command` is the program and its arguments, to which each clip's path is appended.
    pub(crate) fn new(command: &str) -> Self {
        Self {
            command: command.to_string(),
            current: None,
        }
    }

    pub(crate) fn stop(&mut self) {
        if let Some(playback) = self.current.take() {
            playback.stop();
        }
    }

    /// Plays `clips` in order, after stopping what was playing.
    pub(crate) fn play(&mut self, clips: &[String]) {
        self.stop();
        let clips = clips
            .iter()
            .filter(|clip| {
                let exists = Path::new(clip).is_file();
                if !exists {
                    println!("Audio clip not found: {clip}");
                }
                exists
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut parts = self.command.split_whitespace();
        let Some(program) = parts.next().map(ToString::to_string) else {
            return;
        };
        if clips.is_empty() {
            return;
        }
        let args = parts.map(ToString::to_string).collect::<Vec<_>>();
        let playback = Playback {
            stopped: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
        };
        let stopped = playback.stopped.clone();
        let current_child = playback.child.clone();
        std::thread::spawn(move || {
            for clip in clips {
                let mut child_slot = current_child.lock().unwrap();
                if stopped.load(Ordering::SeqCst) {
                    return;
                }
                let spawned = Command::new(&program)
                    .args(&args)
                    .arg(&clip)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                match spawned {
                    Ok(child) => *child_slot = Some(child),
                    Err(e) => {
                        println!("Failed to play {clip} with `{program}`: {e}");
                        return;
                    }
                }
                drop(child_slot);
                // Polled, so `stop` can take the lock to kill the child while it plays
                loop {
                    let mut child_slot = current_child.lock().unwrap();
                    let Some(child) = child_slot.as_mut() else {
                        return;
                    };
                    if !matches!(child.try_wait(), Ok(None)) {
                        *child_slot = None;
                        break;
                    }
                    drop(child_slot);
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            }
        });
        self.current = Some(playback);
    }

    /// Plays the clips `card_flipped`'s side reveals: the front's, or those the back adds.
    pub(crate) fn autoplay(&mut self, audio: Option<&CardAudio>, card_flipped: bool) {
        match audio {
            Some(audio) => self.play(if card_flipped {
                &audio.back
            } else {
                &audio.front
            }),
            None => self.stop(),
        }
    }

    /// Plays the front's clips, and then the back's if `card_flipped`.
    pub(crate) fn replay(&mut self, audio: Option<&CardAudio>, card_flipped: bool) {
        let Some(audio) = audio else {
            return;
        };
        let mut clips = audio.front.clone();
        if card_flipped {
            clips.extend(audio.back.iter().cloned());
        }
        self.play(&clips);
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use std::time::Instant;

    use super::*;

    #[test]
    fn test_play_in_order_and_stop() {
        let dir = std::env::temp_dir().join(format!("spares-audio-player-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let clips = ["a.mp3", "b.mp3"].map(|name| {
            let path = dir.join(name);
            std::fs::write(&path, "").unwrap();
            path.display().to_string()
        });
        let log = dir.join("log");
        let script = dir.join("player.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$1\" >> {}\n", log.display()),
        )
        .unwrap();
        let mut player = AudioPlayer::new(&format!("sh {}", script.display()));
        let missing = dir.join("missing.mp3").display().to_string();
        player.play(&[clips[0].clone(), missing, clips[1].clone()]);
        let start = Instant::now();
        while std::fs::read_to_string(&log).map_or(0, |log| log.lines().count()) < 2 {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "clips were not played"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            format!("{}\n{}\n", clips[0], clips[1])
        );

        // Stopping kills the clip playing and skips the rest
        let sleeper = dir.join("sleeper.sh");
        std::fs::write(&sleeper, "#!/bin/sh\nsleep 5\n").unwrap();
        let mut player = AudioPlayer::new(&format!("sh {}", sleeper.display()));
        player.play(&clips);
        std::thread::sleep(Duration::from_millis(200));
        let start = Instant::now();
        drop(player);
        assert!(start.elapsed() < Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
