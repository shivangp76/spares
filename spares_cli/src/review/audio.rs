//! Plays a card's audio clips with the configured `audio_player`, and reads its text aloud with
//! `tts_command`, without waiting for them.

use std::path::Path;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use spares_core::parsers::audio::AudioItem;
use spares_core::parsers::audio::CardAudio;
use spares_core::parsers::audio::Speech;

/// Plays clips one after another in the background. Playing again, or dropping it, stops what
/// was playing.
pub(crate) struct AudioPlayer {
    command: String,
    tts_command: Option<String>,
    /// Whether it was said that text is not read without `tts_command`
    told_no_tts: bool,
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
    /// `tts_command` reads text aloud (see `tts_command` in `config.toml`).
    pub(crate) fn new(command: &str, tts_command: Option<&str>) -> Self {
        Self {
            command: command.to_string(),
            tts_command: tts_command.map(ToString::to_string),
            told_no_tts: false,
            current: None,
        }
    }

    pub(crate) fn stop(&mut self) {
        if let Some(playback) = self.current.take() {
            playback.stop();
        }
    }

    /// Plays `items` in order, after stopping what was playing.
    pub(crate) fn play(&mut self, items: &[AudioItem]) {
        self.stop();
        let mut commands: Vec<(String, Vec<String>)> = Vec::new();
        for item in items {
            match item {
                AudioItem::Clip(clip) => {
                    if !Path::new(clip).is_file() {
                        println!("Audio clip not found: {clip}");
                        continue;
                    }
                    let mut args = self
                        .command
                        .split_whitespace()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>();
                    if !args.is_empty() {
                        args.push(clip.clone());
                        commands.push((args.remove(0), args));
                    }
                }
                AudioItem::Speech(speech) => match &self.tts_command {
                    Some(tts_command) => {
                        let mut args = tts_args(tts_command, speech);
                        if !args.is_empty() {
                            commands.push((args.remove(0), args));
                        }
                    }
                    None if !self.told_no_tts => {
                        println!("Set `tts_command` in config.toml to read the card's text aloud.");
                        self.told_no_tts = true;
                    }
                    None => {}
                },
            }
        }
        if commands.is_empty() {
            return;
        }
        let playback = Playback {
            stopped: Arc::new(AtomicBool::new(false)),
            child: Arc::new(Mutex::new(None)),
        };
        let stopped = playback.stopped.clone();
        let current_child = playback.child.clone();
        std::thread::spawn(move || {
            for (program, args) in commands {
                let mut child_slot = current_child.lock().unwrap();
                if stopped.load(Ordering::SeqCst) {
                    return;
                }
                let spawned = Command::new(&program)
                    .args(&args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                match spawned {
                    Ok(child) => *child_slot = Some(child),
                    Err(e) => {
                        println!("Failed to play audio with `{program}`: {e}");
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

    /// Plays what `card_flipped`'s side reveals: the front's, or those the back adds.
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

    /// Plays the front's audio, and then the back's if `card_flipped`.
    pub(crate) fn replay(&mut self, audio: Option<&CardAudio>, card_flipped: bool) {
        let Some(audio) = audio else {
            return;
        };
        let mut items = audio.front.clone();
        if card_flipped {
            items.extend(audio.back.iter().cloned());
        }
        self.play(&items);
    }
}

/// The program and arguments that read `speech` aloud with `tts_command` (see `tts_command` in
/// `config.toml`).
fn tts_args(tts_command: &str, speech: &Speech) -> Vec<String> {
    let Speech { text, voice } = speech;
    let values = [
        ("{text}", Some(text.clone())),
        (
            "{lang}",
            voice.lang.as_ref().map(|lang| lang.replace('_', "-")),
        ),
        ("{voice}", voice.voice.clone()),
        ("{speed}", voice.speed.map(|speed| speed.to_string())),
    ];
    let mut args = tts_command
        .split_whitespace()
        .filter_map(|arg| {
            let mut arg = arg.to_string();
            for (placeholder, value) in &values {
                if arg.contains(placeholder) {
                    arg = arg.replace(placeholder, value.as_deref()?);
                }
            }
            Some(arg)
        })
        .collect::<Vec<_>>();
    if !tts_command.contains("{text}") {
        args.push(text.clone());
    }
    args
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

    use spares_core::parsers::tts::TtsVoice;

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
        let mut player = AudioPlayer::new(&format!("sh {}", script.display()), None);
        let missing = dir.join("missing.mp3").display().to_string();
        player.play(&[clips[0].clone(), missing, clips[1].clone()].map(AudioItem::Clip));
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
        let mut player = AudioPlayer::new(&format!("sh {}", sleeper.display()), None);
        player.play(&clips.clone().map(AudioItem::Clip));
        std::thread::sleep(Duration::from_millis(200));
        let start = Instant::now();
        drop(player);
        assert!(start.elapsed() < Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_tts_args() {
        let speech = |lang: Option<&str>, voice: Option<&str>| Speech {
            text: "bonjour à tous".to_string(),
            voice: TtsVoice {
                lang: lang.map(ToString::to_string),
                speed: Some(0.8),
                voice: voice.map(ToString::to_string),
            },
        };
        assert_eq!(
            tts_args("say --voice={voice} {text}", &speech(None, Some("Thomas"))),
            ["say", "--voice=Thomas", "bonjour à tous"]
        );
        // An argument without its value is left out
        assert_eq!(
            tts_args("say --voice={voice} {text}", &speech(Some("fr_FR"), None)),
            ["say", "bonjour à tous"]
        );
        // Without `{text}`, the text comes last
        assert_eq!(
            tts_args(
                "espeak-ng -v {lang} -s {speed}",
                &speech(Some("fr_FR"), None)
            ),
            ["espeak-ng", "-v", "fr-FR", "-s", "0.8", "bonjour à tous"]
        );
    }
}
