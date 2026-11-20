//! The presets `emulate` may name, and the binding of one to an `emulate`
//! value.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use http::HeaderValue;

use super::{Emulation, Preset};
use crate::config::EmulateConfig;
use crate::text::short;

/// What `emulate` may name: the built-in presets, and the `*.toml` files of
/// one `presets/` directory.
#[derive(Debug, Clone)]
pub struct PresetLibrary {
    presets: BTreeMap<String, Entry>,
    /// Each starts with its file: `presets/mine.toml: headers: …`.
    problems: Vec<String>,
}

#[derive(Debug, Clone)]
enum Entry {
    Loaded(Arc<Preset>),
    /// Its problems are in `problems`.
    Broken,
}

/// One reason an `emulate` value selects nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindProblem {
    /// The variable at fault; `None` when it is the preset name.
    pub variable: Option<String>,
    pub problem: String,
}

impl BindProblem {
    /// The problem as a line about the `emulate` key written at `field`.
    pub fn at(&self, field: &str) -> String {
        match &self.variable {
            Some(variable) => format!("{field}.{}: {}", short(variable), self.problem),
            None => format!("{field}: {}", self.problem),
        }
    }
}

/// The presets compiled into the binary, by name.
const BUILT_IN: [(&str, &str); 1] = [("opencode", include_str!("opencode.toml"))];

static BUILT_IN_PRESETS: LazyLock<Vec<Arc<Preset>>> = LazyLock::new(|| {
    BUILT_IN
        .iter()
        .map(|(name, text)| {
            let preset = Preset::parse(name, text)
                .unwrap_or_else(|problems| panic!("built-in preset `{name}`: {problems:?}"));
            Arc::new(preset)
        })
        .collect()
});

/// The built-in presets alone.
impl Default for PresetLibrary {
    fn default() -> Self {
        Self {
            presets: BUILT_IN_PRESETS
                .iter()
                .map(|preset| (preset.name.clone(), Entry::Loaded(Arc::clone(preset))))
                .collect(),
            problems: Vec::new(),
        }
    }
}

impl PresetLibrary {
    /// The built-in presets and every `*.toml` file in `dir`, each the preset
    /// its file name says. Never fails: a file that cannot be read, has a
    /// problem or is named like a built-in preset is listed in
    /// [`Self::problems`]. A directory that does not exist adds no preset.
    pub fn with_dir(dir: &Path) -> Self {
        let mut library = Self::default();
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return library,
            Err(error) => {
                library
                    .problems
                    .push(format!("presets/: cannot read: {error}"));
                return library;
            }
        };
        let mut files: Vec<PathBuf> = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => files.push(entry.path()),
                Err(error) => library
                    .problems
                    .push(format!("presets/: cannot read: {error}")),
            }
        }
        files.retain(|file| file.extension().is_some_and(|ext| ext == "toml") && file.is_file());
        files.sort();
        for file in files {
            let Some(name) = file.file_stem().and_then(|stem| stem.to_str()) else {
                library.problems.push(format!(
                    "presets/{}: the file name is not UTF-8",
                    short(&file.file_name().unwrap_or_default().to_string_lossy())
                ));
                continue;
            };
            if BUILT_IN.iter().any(|(built_in, _)| *built_in == name) {
                library.problems.push(format!(
                    "presets/{name}.toml: `{name}` is a built-in preset; give the file another name"
                ));
                continue;
            }
            let preset = std::fs::read_to_string(&file)
                .map_err(|error| vec![format!("cannot read: {error}")])
                .and_then(|text| Preset::parse(name, &text));
            let entry = match preset {
                Ok(preset) => Entry::Loaded(Arc::new(preset)),
                Err(problems) => {
                    let shown = short(name);
                    library.problems.extend(
                        problems
                            .iter()
                            .map(|problem| format!("presets/{shown}.toml: {problem}")),
                    );
                    Entry::Broken
                }
            };
            library.presets.insert(name.to_owned(), entry);
        }
        library
    }

    /// What is wrong with the preset files, one line each.
    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    /// The preset `emulate` names with the values it gives. `Err` holds what
    /// is wrong with `emulate`; it is empty when the preset's file has
    /// problems, since [`Self::problems`] already lists those.
    pub fn bind(&self, emulate: &EmulateConfig) -> Result<Emulation, Vec<BindProblem>> {
        let preset = match self.presets.get(&emulate.preset) {
            Some(Entry::Loaded(preset)) => preset,
            Some(Entry::Broken) => return Err(Vec::new()),
            None => {
                return Err(vec![BindProblem {
                    variable: None,
                    problem: format!(
                        "`{}` is not a preset; {}",
                        short(&emulate.preset),
                        self.known()
                    ),
                }]);
            }
        };
        let mut problems = Vec::new();
        for (variable, value) in &emulate.values {
            let problem = if !preset.variables.contains_key(variable) {
                format!(
                    "preset `{}` declares no variable `{}`",
                    short(&emulate.preset),
                    short(variable)
                )
            } else if preset.sends_in_a_header(variable) && HeaderValue::from_str(value).is_err() {
                "the value cannot be sent in a header".to_owned()
            } else {
                continue;
            };
            problems.push(BindProblem {
                variable: Some(variable.clone()),
                problem,
            });
        }
        if !problems.is_empty() {
            return Err(problems);
        }
        let mut values = preset.variables.clone();
        values.extend(emulate.values.clone());
        Ok(Emulation {
            preset: Arc::clone(preset),
            values,
        })
    }

    /// The names `emulate` may use, for a message.
    fn known(&self) -> String {
        let names: Vec<String> = self
            .presets
            .keys()
            .map(|name| format!("`{}`", short(name)))
            .collect();
        format!("there are: {}", names.join(", "))
    }
}
