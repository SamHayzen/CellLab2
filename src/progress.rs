use crate::model::Gene;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
pub struct Progress {
    pub completed: BTreeSet<String>,
    #[serde(default)]
    pub blueprint: Option<Vec<Gene>>,
    #[serde(default = "enabled_by_default")]
    pub music_enabled: bool,
    #[serde(default = "enabled_by_default")]
    pub effects_enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            completed: BTreeSet::new(),
            blueprint: None,
            music_enabled: true,
            effects_enabled: true,
        }
    }
}

impl Progress {
    pub fn load() -> Result<Self, String> {
        let path = progress_path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = fs::read(path).map_err(|error| error.to_string())?;
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&progress_path()?)
    }

    fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|error| error.to_string())?;
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
        if path.exists() {
            let backup = path.with_extension("json.bak");
            fs::copy(path, &backup).map_err(|error| error.to_string())?;
            if let Err(error) = fs::rename(&temporary, path) {
                let _ = fs::copy(&backup, path);
                return Err(error.to_string());
            }
        } else {
            fs::rename(&temporary, path).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}

fn progress_path() -> Result<PathBuf, String> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::current_exe()
                .ok()?
                .parent()
                .map(Path::to_path_buf)
        })
        .ok_or("Cannot locate the progress directory")?;
    Ok(base.join("CellLab2").join("progress.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_levels_survive_save_and_load() {
        let dir = std::env::current_dir().unwrap().join("target");
        let path = dir.join(format!("progress-test-{}.json", std::process::id()));
        let mut progress = Progress::default();
        progress.completed.insert("30".into());
        progress.music_enabled = false;
        progress.effects_enabled = false;
        progress.save_to(&path).unwrap();
        progress.completed.insert("31".into());
        progress.save_to(&path).unwrap();
        let restored: Progress = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(restored.completed.contains("30"));
        assert!(restored.completed.contains("31"));
        assert!(!restored.music_enabled);
        assert!(!restored.effects_enabled);
        fs::remove_file(&path).unwrap();
        fs::remove_file(path.with_extension("json.bak")).unwrap();
    }

    #[test]
    fn synthesizer_genome_survives_progress_save() {
        let path = std::env::current_dir()
            .unwrap()
            .join("target")
            .join(format!("blueprint-test-{}.json", std::process::id()));
        let mut progress = Progress::default();
        let mut genes = crate::legacy::import(crate::bundled::PLATES[0].1)
            .unwrap()
            .cells[0]
            .genes
            .clone();
        genes[3].flags[4] = true;
        progress.blueprint = Some(genes);
        progress.save_to(&path).unwrap();
        let restored: Progress = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(restored.blueprint.unwrap()[3].flags[4]);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn older_progress_files_enable_original_audio_defaults() {
        let progress: Progress = serde_json::from_str(r#"{"completed":[]}"#).unwrap();
        assert!(progress.music_enabled);
        assert!(progress.effects_enabled);
    }
}
