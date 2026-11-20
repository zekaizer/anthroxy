//! A configuration file on disk with the preset files beside it.

use std::path::PathBuf;

/// A temporary directory holding `config.toml` and `presets/`.
pub struct ConfigDir(tempfile::TempDir);

impl ConfigDir {
    /// `presets` are `(name, text)`: each becomes `presets/<name>.toml`.
    pub fn new(config: &str, presets: &[(&str, &str)]) -> Self {
        let dir = Self(tempfile::tempdir().unwrap());
        dir.write_config(config);
        for (name, text) in presets {
            dir.write_preset(name, text);
        }
        dir
    }

    pub fn config(&self) -> PathBuf {
        self.0.path().join("config.toml")
    }

    pub fn write_config(&self, text: &str) {
        std::fs::write(self.config(), text).unwrap();
    }

    pub fn write_preset(&self, name: &str, text: &str) {
        let presets = self.0.path().join("presets");
        std::fs::create_dir_all(&presets).unwrap();
        std::fs::write(presets.join(format!("{name}.toml")), text).unwrap();
    }
}
