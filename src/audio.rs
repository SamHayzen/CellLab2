//! The APK's MP3 effects and music played through Windows MCI. The assets stay
//! embedded in the executable and are unpacked only for the running process.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Track {
    Experiment,
    Challenge,
}

#[derive(Clone, Copy)]
pub enum Sound {
    Button,
    Deselect,
    Invalid,
    Place,
    Save,
    Select,
    Sterilize,
    Tutorial,
    Win,
}

impl Sound {
    fn key(self) -> &'static str {
        match self {
            Self::Button => "button",
            Self::Deselect => "deselect_cell",
            Self::Invalid => "invalid",
            Self::Place => "place_cell",
            Self::Save => "save",
            Self::Select => "select_cell",
            Self::Sterilize => "sterilize",
            Self::Tutorial => "tut_good",
            Self::Win => "win",
        }
    }
}

impl Track {
    fn key(self) -> &'static str {
        match self {
            Self::Experiment => "experiment_song",
            Self::Challenge => "challenge_song",
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::{Sound, Track};
    use std::{fs, path::PathBuf, ptr};

    #[link(name = "winmm")]
    unsafe extern "system" {
        fn mciSendStringW(
            command: *const u16,
            result: *mut u16,
            result_len: u32,
            callback: usize,
        ) -> u32;
    }

    const ASSETS: &[(&str, &[u8])] = &[
        (
            "experiment_song.mp3",
            include_bytes!("../assets/audio/experiment_song.mp3"),
        ),
        (
            "challenge_song.mp3",
            include_bytes!("../assets/audio/challenge_song.mp3"),
        ),
        (
            "button.mp3",
            include_bytes!("../assets/audio/button.mp3"),
        ),
        (
            "deselect_cell.mp3",
            include_bytes!("../assets/audio/deselect_cell.mp3"),
        ),
        (
            "invalid.mp3",
            include_bytes!("../assets/audio/invalid.mp3"),
        ),
        (
            "place_cell.mp3",
            include_bytes!("../assets/audio/place_cell.mp3"),
        ),
        (
            "save.mp3",
            include_bytes!("../assets/audio/save.mp3"),
        ),
        (
            "select_cell.mp3",
            include_bytes!("../assets/audio/select_cell.mp3"),
        ),
        (
            "sterilize.mp3",
            include_bytes!("../assets/audio/sterilize.mp3"),
        ),
        (
            "tut_good.mp3",
            include_bytes!("../assets/audio/tut_good.mp3"),
        ),
        (
            "win.mp3",
            include_bytes!("../assets/audio/win.mp3"),
        ),
    ];

    fn command(value: &str) -> Result<(), String> {
        let wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        // MCI reads the null-terminated command during this synchronous call.
        let code = unsafe { mciSendStringW(wide.as_ptr(), ptr::null_mut(), 0, 0) };
        if code == 0 {
            Ok(())
        } else {
            Err(format!("Windows audio command failed ({code}): {value}"))
        }
    }

    pub struct Audio {
        directory: PathBuf,
        files: Vec<PathBuf>,
        opened: Vec<String>,
        prefix: String,
        track: Track,
        music_enabled: bool,
        effects_enabled: bool,
    }

    impl Audio {
        pub fn new(music_enabled: bool, effects_enabled: bool) -> Result<Self, String> {
            let pid = std::process::id();
            let directory = std::env::temp_dir().join(format!("CellLab2-audio-{pid}"));
            fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
            let mut audio = Self {
                directory,
                files: Vec::new(),
                opened: Vec::new(),
                prefix: format!("celllab2_{pid}"),
                track: Track::Experiment,
                music_enabled,
                effects_enabled,
            };
            for &(name, bytes) in ASSETS {
                let path = audio.directory.join(name);
                fs::write(&path, bytes).map_err(|error| error.to_string())?;
                audio.files.push(path.clone());
                let key = name.trim_end_matches(".mp3");
                let alias = audio.alias(key);
                command(&format!(
                    "open \"{}\" type mpegvideo alias {alias}",
                    path.display()
                ))?;
                audio.opened.push(alias);
            }
            for track in [Track::Experiment, Track::Challenge] {
                let _ = command(&format!(
                    "setaudio {} volume to 500",
                    audio.alias(track.key())
                ));
            }
            if music_enabled {
                audio.start_music();
            }
            Ok(audio)
        }

        fn alias(&self, key: &str) -> String {
            format!("{}_{}", self.prefix, key)
        }

        fn start_music(&self) {
            let alias = self.alias(self.track.key());
            let _ = command(&format!("seek {alias} to start"));
            let _ = command(&format!("play {alias} repeat"));
        }

        pub fn set_track(&mut self, track: Track) {
            if self.track == track {
                return;
            }
            if self.music_enabled {
                let _ = command(&format!("stop {}", self.alias(self.track.key())));
            }
            self.track = track;
            if self.music_enabled {
                self.start_music();
            }
        }

        pub fn set_music_enabled(&mut self, enabled: bool) {
            if self.music_enabled == enabled {
                return;
            }
            self.music_enabled = enabled;
            if enabled {
                self.start_music();
            } else {
                let _ = command(&format!("stop {}", self.alias(self.track.key())));
            }
        }

        pub fn set_effects_enabled(&mut self, enabled: bool) {
            self.effects_enabled = enabled;
        }

        pub fn play(&self, sound: Sound) {
            if !self.effects_enabled {
                return;
            }
            let alias = self.alias(sound.key());
            let _ = command(&format!("stop {alias}"));
            let _ = command(&format!("seek {alias} to start"));
            let _ = command(&format!("play {alias}"));
        }
    }

    impl Drop for Audio {
        fn drop(&mut self) {
            for alias in &self.opened {
                let _ = command(&format!("close {alias}"));
            }
            for path in &self.files {
                let _ = fs::remove_file(path);
            }
            let _ = fs::remove_dir(&self.directory);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn original_audio_assets_open_without_playback() {
            let audio = Audio::new(false, false).unwrap();
            assert_eq!(audio.opened.len(), ASSETS.len());
        }
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    use super::{Sound, Track};

    pub struct Audio;

    impl Audio {
        pub fn new(_: bool, _: bool) -> Result<Self, String> {
            Ok(Self)
        }
        pub fn set_track(&mut self, _: Track) {}
        pub fn set_music_enabled(&mut self, _: bool) {}
        pub fn set_effects_enabled(&mut self, _: bool) {}
        pub fn play(&self, _: Sound) {}
    }
}

pub use platform::Audio;
