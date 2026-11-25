//! User palettes and independent settings templates in a versioned TOML file.

use crate::settings::{Palettes, Preset, Settings, Template, Templates};
use rthid_dither::dither::{Mode, Tone};
use rthid_io::atomic;
use serde::{Deserialize, Serialize};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// A named edit applied to the latest configuration when saving.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Replaces a user preset by name or appends it.
    UpsertPreset(Preset),
    /// Removes a user preset by name if present.
    RemovePreset(String),
    /// Replaces a template by name or appends it.
    UpsertTemplate(Template),
    /// Removes a template by name if present.
    RemoveTemplate(String),
}

impl Change {
    /// Identifies the list and name replaced by this edit.
    pub fn key(&self) -> (&str, &str) {
        match self {
            Self::UpsertPreset(preset) => ("palette", &preset.name),
            Self::RemovePreset(name) => ("palette", name),
            Self::UpsertTemplate(template) => ("template", &template.name),
            Self::RemoveTemplate(name) => ("template", name),
        }
    }
}

/// Parsed entries and diagnostics that protect an unreadable file from replacement.
pub struct Loaded {
    /// Built-ins followed by valid user presets.
    pub palettes: Palettes,
    /// Valid settings snapshots.
    pub templates: Templates,
    /// One diagnostic for each skipped entry.
    pub warnings: Vec<String>,
    /// Reason saving must remain disabled for this session.
    pub save_blocked: Option<String>,
}

impl Loaded {
    /// Summarizes skipped entries and blocked saving for the status line.
    pub fn notice(&self) -> Option<String> {
        let mut messages = Vec::new();
        if let Some(first) = self.warnings.first() {
            messages.push(format!("{} entries skipped: {first}", self.warnings.len()));
        }
        if let Some(reason) = &self.save_blocked {
            messages.push(format!(
                "saving disabled: {}",
                reason.lines().next().unwrap_or(reason)
            ));
        }
        (!messages.is_empty()).then(|| format!("config: {}", messages.join(" | ")))
    }
}

impl Default for Loaded {
    fn default() -> Self {
        Self {
            palettes: Palettes::builtin(),
            templates: Templates::default(),
            warnings: Vec::new(),
            save_blocked: None,
        }
    }
}

/// Uses built-ins and disables saving when no configuration path is available.
pub fn load_or_builtin(path: Option<&Path>) -> Loaded {
    path.map_or_else(
        || Loaded {
            save_blocked: Some("no configuration path available".into()),
            ..Loaded::default()
        },
        load,
    )
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum FileMode {
    Tone,
    Color,
}
impl From<Mode> for FileMode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Tone => Self::Tone,
            Mode::Color => Self::Color,
        }
    }
}
impl From<FileMode> for Mode {
    fn from(mode: FileMode) -> Self {
        match mode {
            FileMode::Tone => Self::Tone,
            FileMode::Color => Self::Color,
        }
    }
}

#[derive(Deserialize)]
struct FileStore {
    version: u32,
}
#[derive(Serialize, Deserialize)]
struct FilePreset {
    name: String,
    colors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mode: Option<FileMode>,
}
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct FileTemplate {
    name: String,
    palette: String,
    colors: Vec<String>,
    mode: FileMode,
    strength: f64,
    serpentine: bool,
    scale: i32,
    tone: FileTone,
}
impl Default for FileTemplate {
    fn default() -> Self {
        Self::from(Template::capture(String::new(), &Settings::default()))
    }
}
impl From<Template> for FileTemplate {
    fn from(t: Template) -> Self {
        Self {
            name: t.name,
            palette: t.palette_name,
            colors: t.palette_hex,
            mode: t.mode.into(),
            strength: t.strength,
            serpentine: t.serpentine,
            scale: t.scale,
            tone: t.tone.into(),
        }
    }
}
impl From<FileTemplate> for Template {
    fn from(t: FileTemplate) -> Self {
        Self {
            name: t.name,
            palette_name: t.palette,
            palette_hex: t.colors,
            mode: t.mode.into(),
            strength: t.strength,
            serpentine: t.serpentine,
            scale: t.scale,
            tone: t.tone.into(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct FileTone {
    brightness: f64,
    contrast: f64,
    gamma: f64,
    black: f64,
    white: f64,
    sharpen: f64,
}
impl Default for FileTone {
    fn default() -> Self {
        Tone::default().into()
    }
}
impl From<Tone> for FileTone {
    fn from(t: Tone) -> Self {
        Self {
            brightness: t.brightness,
            contrast: t.contrast,
            gamma: t.gamma,
            black: t.black,
            white: t.white,
            sharpen: t.sharpen,
        }
    }
}
impl From<FileTone> for Tone {
    fn from(t: FileTone) -> Self {
        Self {
            brightness: t.brightness,
            contrast: t.contrast,
            gamma: t.gamma,
            black: t.black,
            white: t.white,
            sharpen: t.sharpen,
        }
    }
}

/// Loads valid entries while retaining diagnostics and preventing unsafe replacement.
pub fn load(path: &Path) -> Loaded {
    let read = || {
        if !fs::metadata(path)?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "config is not a regular file",
            ));
        }
        fs::read_to_string(path)
    };
    let text = match read() {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Loaded::default(),
        Err(error) => {
            return Loaded {
                save_blocked: Some(error.to_string()),
                ..Loaded::default()
            };
        }
    };
    parse(&text)
}

/// Parses valid entries and retains warnings and reasons to block saving.
pub fn parse(text: &str) -> Loaded {
    if text.trim().is_empty() {
        return Loaded::default();
    }
    let mut table: toml::Table = match toml::from_str(text) {
        Ok(table) => table,
        Err(error) => {
            return Loaded {
                save_blocked: Some(error.to_string()),
                ..Loaded::default()
            };
        }
    };
    let palettes = table
        .remove("palette")
        .unwrap_or(toml::Value::Array(Vec::new()));
    let templates = table
        .remove("template")
        .unwrap_or(toml::Value::Array(Vec::new()));
    table.entry("version").or_insert(toml::Value::Integer(1));
    let file: FileStore = match toml::Value::Table(table).try_into() {
        Ok(file) => file,
        Err(error) => {
            return Loaded {
                save_blocked: Some(error.to_string()),
                ..Loaded::default()
            };
        }
    };
    let (Some(palettes), Some(templates)) = (palettes.as_array(), templates.as_array()) else {
        return Loaded {
            save_blocked: Some("palette and template must be arrays".into()),
            ..Loaded::default()
        };
    };
    let mut loaded = Loaded::default();
    if file.version > 1 {
        loaded.save_blocked = Some(format!("unsupported config version {}", file.version));
    }
    for value in palettes {
        let entry: FilePreset = match value.clone().try_into() {
            Ok(entry) => entry,
            Err(error) => {
                loaded.warnings.push(format!("palette: {error}"));
                continue;
            }
        };
        let preset = Preset {
            name: entry.name,
            colors: entry.colors,
            mode: entry.mode.map(Into::into),
            builtin: false,
        };
        let name = preset.name.clone();
        let result = if loaded.palettes.find(&name).is_some() {
            Err("duplicate or built-in name".into())
        } else {
            loaded.palettes.upsert(preset)
        };
        if let Err(error) = result {
            loaded.warnings.push(format!("palette {name:?}: {error}"));
        }
    }
    for value in templates {
        let entry: FileTemplate = match value.clone().try_into() {
            Ok(entry) => entry,
            Err(error) => {
                loaded.warnings.push(format!("template: {error}"));
                continue;
            }
        };
        let template = Template::from(entry);
        let name = template.name.clone();
        let result = if loaded.templates.find(&name).is_some() {
            Err("duplicate name".into())
        } else {
            loaded.templates.upsert(template)
        };
        if let Err(error) = result {
            loaded.warnings.push(format!("template {name:?}: {error}"));
        }
    }
    loaded
}

/// Merges named edits into fresh text under the config lock and returns the saved entries.
/// Untouched raw values retain their array order, including entries this version cannot parse.
/// TOML comments and formatting are lost on save.
pub fn save(path: &Path, changes: &[Change]) -> io::Result<Loaded> {
    atomic::update(path, |text| {
        let text = text.filter(|text| !text.trim().is_empty());
        let mut loaded = text.as_deref().map(parse).unwrap_or_default();
        if let Some(reason) = &loaded.save_blocked {
            return Err(io::Error::other(reason.clone()));
        }
        let mut table: toml::Table = text
            .as_deref()
            .map(toml::from_str)
            .transpose()
            .map_err(io::Error::other)?
            .unwrap_or_default();
        table.entry("version").or_insert(toml::Value::Integer(1));
        for change in changes {
            match change {
                Change::UpsertPreset(preset) => {
                    loaded
                        .palettes
                        .upsert(preset.clone())
                        .map_err(io::Error::other)?;
                }
                Change::RemovePreset(name) => {
                    if let Some(index) = loaded.palettes.find(name) {
                        loaded.palettes.remove(index).map_err(io::Error::other)?;
                    }
                }
                Change::UpsertTemplate(template) => {
                    loaded
                        .templates
                        .upsert(template.clone())
                        .map_err(io::Error::other)?;
                }
                Change::RemoveTemplate(name) => {
                    if let Some(index) = loaded.templates.find(name) {
                        loaded.templates.remove(index);
                    }
                }
            }
        }
        for change in changes {
            let (kind, name, replacement) = match change {
                Change::UpsertPreset(preset) => (
                    "palette",
                    preset.name.as_str(),
                    Some(
                        toml::Value::try_from(FilePreset {
                            name: preset.name.clone(),
                            colors: preset.colors.clone(),
                            mode: preset.mode.map(Into::into),
                        })
                        .map_err(io::Error::other)?,
                    ),
                ),
                Change::RemovePreset(name) => ("palette", name.as_str(), None),
                Change::UpsertTemplate(template) => (
                    "template",
                    template.name.as_str(),
                    Some(
                        toml::Value::try_from(FileTemplate::from(template.clone()))
                            .map_err(io::Error::other)?,
                    ),
                ),
                Change::RemoveTemplate(name) => ("template", name.as_str(), None),
            };
            let entries = table
                .entry(kind)
                .or_insert(toml::Value::Array(Vec::new()))
                .as_array_mut()
                .unwrap();
            let position = entries
                .iter()
                .position(|entry| entry.get("name").and_then(toml::Value::as_str) == Some(name));
            // The UI shows one entry per name, so edits replace or remove even invalid duplicates.
            entries.retain(|entry| entry.get("name").and_then(toml::Value::as_str) != Some(name));
            if let Some(value) = replacement {
                entries.insert(position.unwrap_or(entries.len()).min(entries.len()), value);
            }
        }
        let text = toml::to_string_pretty(&table).map_err(io::Error::other)?;
        let loaded = parse(&text);
        Ok((text, loaded))
    })
}

/// Resolves the explicit path before environment overrides and the home directory.
pub fn config_path(flag: Option<PathBuf>) -> Option<PathBuf> {
    path_from_env(
        flag,
        std::env::var_os("RTHID_CONFIG").map(PathBuf::from),
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn path_from_env(
    flag: Option<PathBuf>,
    config: Option<PathBuf>,
    xdg: Option<PathBuf>,
    home: Option<PathBuf>,
) -> Option<PathBuf> {
    flag.or(config)
        .or_else(|| {
            xdg.filter(|path| path.is_absolute())
                .map(|path| path.join("rthid/config.toml"))
        })
        .or_else(|| home.map(|path| path.join(".config/rthid/config.toml")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_round_trip_preserves_entries_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config.toml");
        assert_eq!(load(&path).palettes, Palettes::builtin());
        assert!(load(&path).templates.is_empty());
        assert!(load(&path).save_blocked.is_none());
        let no_path = load_or_builtin(None);
        assert_eq!(no_path.palettes, Palettes::builtin());
        assert!(no_path.templates.is_empty());
        assert!(no_path.save_blocked.is_some());
        let settings = Settings::default();
        let mut palettes = Palettes::builtin();
        palettes
            .upsert(Preset {
                name: "Paper".into(),
                colors: vec!["#fff".into(), "#123456".into()],
                mode: Some(Mode::Color),
                builtin: false,
            })
            .unwrap();
        let mut templates = Templates::default();
        templates
            .upsert(Template::capture("Zine".into(), &settings))
            .unwrap();
        let changes: Vec<_> = palettes
            .user()
            .cloned()
            .map(Change::UpsertPreset)
            .chain(templates.iter().cloned().map(Change::UpsertTemplate))
            .collect();
        save(&path, &changes).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.palettes, palettes);
        assert_eq!(loaded.templates, templates);
        assert!(loaded.warnings.is_empty());
        assert!(loaded.save_blocked.is_none());
        fs::write(&path, "version = 1\nunknown = true\n[[template]]\nname = 'Defaults'\n[template.tone]\ncontrast = 0.15\n").unwrap();
        let loaded = load(&path);
        let mut expected = Template::capture("Defaults".into(), &settings);
        expected.tone.contrast = 0.15;
        assert_eq!(
            loaded.templates.iter().cloned().collect::<Vec<_>>(),
            vec![expected]
        );
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn empty_and_versionless_files_accept_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let change =
            Change::UpsertTemplate(Template::capture("Saved".into(), &Settings::default()));
        for text in ["", " \n\t", "[[template]]\nname = 'Existing'\n"] {
            fs::write(&path, text).unwrap();
            assert!(load(&path).save_blocked.is_none());
            let loaded = save(&path, std::slice::from_ref(&change)).unwrap();
            assert!(loaded.templates.find("Saved").is_some());
            assert!(loaded.save_blocked.is_none());
            if !text.trim().is_empty() {
                assert!(loaded.templates.find("Existing").is_some());
            }
        }
    }

    #[test]
    fn stale_instances_merge_named_edits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for presets in [true, false] {
            let upsert = |name: &str, color: &str| {
                if presets {
                    Change::UpsertPreset(Preset {
                        name: name.into(),
                        colors: vec![color.into()],
                        mode: None,
                        builtin: false,
                    })
                } else {
                    let mut template = Template::capture(name.into(), &Settings::default());
                    template.palette_hex = vec![color.into()];
                    Change::UpsertTemplate(template)
                }
            };
            let remove = |name: &str| {
                if presets {
                    Change::RemovePreset(name.into())
                } else {
                    Change::RemoveTemplate(name.into())
                }
            };
            let mut first = load(&path);
            let mut second = load(&path);
            assert!(first.palettes.find("First").is_none());
            assert!(second.templates.find("Second").is_none());
            first = save(&path, &[upsert("First", "#000")]).unwrap();
            second = save(&path, &[upsert("Second", "#fff")]).unwrap();
            let names = |loaded: &Loaded| -> Vec<String> {
                if presets {
                    loaded.palettes.user().map(|p| p.name.clone()).collect()
                } else {
                    loaded.templates.iter().map(|t| t.name.clone()).collect()
                }
            };
            assert_eq!(names(&first), ["First"]);
            assert_eq!(names(&second), ["First", "Second"]);
            save(&path, &[remove("First"), remove("Missing")]).unwrap();
            let merged = save(&path, &[upsert("Second", "#123456")]).unwrap();
            assert_eq!(names(&merged), ["Second"]);
            if presets {
                assert_eq!(merged.palettes.user().next().unwrap().colors, ["#123456"]);
            } else {
                assert_eq!(merged.templates.get(0).unwrap().palette_hex, ["#123456"]);
            }
        }
    }

    #[test]
    fn unrelated_saves_preserve_raw_entries_and_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let original = "version = 1\nunknown = { enabled = true }\n[[palette]]\nname = 'Bad'\ncolors = 42\n[[palette]]\nname = 'Good'\ncolors = ['#fff']\n[[template]]\nname = 'Broken'\nmode = 'future'\n";
        fs::write(&path, original).unwrap();
        let before: toml::Table = toml::from_str(original).unwrap();
        let loaded = save(
            &path,
            &[Change::UpsertTemplate(Template::capture(
                "New".into(),
                &Settings::default(),
            ))],
        )
        .unwrap();
        assert_eq!(loaded.warnings.len(), 2);
        let after: toml::Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["unknown"], before["unknown"]);
        assert_eq!(after["palette"], before["palette"]);
        assert_eq!(after["template"][0], before["template"][0]);
        save(
            &path,
            &[
                Change::RemovePreset("Bad".into()),
                Change::UpsertTemplate(Template::capture("Broken".into(), &Settings::default())),
            ],
        )
        .unwrap();
        assert!(load(&path).warnings.is_empty());
    }

    #[test]
    fn save_rechecks_current_file_and_recovers_after_repair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let change = Change::UpsertTemplate(Template::capture("Zine".into(), &Settings::default()));
        save(&path, &[]).unwrap();
        for text in ["version = [", "version = 2\n"] {
            fs::write(&path, text).unwrap();
            assert!(save(&path, std::slice::from_ref(&change)).is_err());
            assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
            fs::write(&path, "version = 1\n").unwrap();
            let loaded = save(&path, std::slice::from_ref(&change)).unwrap();
            assert!(loaded.templates.find("Zine").is_some());
            assert!(loaded.save_blocked.is_none());
        }
    }

    #[test]
    fn invalid_entries_are_skipped_individually() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for kind in ["palette", "template"] {
            for entry in [
                "name = 'Bad'\ncolors = ['oops']".to_owned(),
                "name = ''\ncolors = ['#fff']".to_owned(),
                "name = 'Black/white'\ncolors = ['#fff']".to_owned(),
                "name = 'Good'\ncolors = ['#fff']".to_owned(),
                "name = 'Empty'\ncolors = []".to_owned(),
                format!("name = '{}'\ncolors = ['#fff']", "猫".repeat(33)),
                format!("name = 'Large'\ncolors = {:?}", vec!["#fff"; 17]),
            ] {
                fs::write(&path, format!("version = 1\n[[{kind}]]\nname = 'Good'\ncolors = ['#000']\n[[{kind}]]\n{entry}\n[[{kind}]]\nname = 'Last'\ncolors = ['#fff']\n")).unwrap();
                let loaded = load(&path);
                let names: Vec<_> = if kind == "palette" {
                    loaded.palettes.user().map(|p| p.name.as_str()).collect()
                } else {
                    loaded.templates.iter().map(|t| t.name.as_str()).collect()
                };
                assert_eq!(names, ["Good", "Last"]);
                assert_eq!(loaded.warnings.len(), 1);
                assert!(loaded.save_blocked.is_none());
            }
        }
    }

    #[test]
    fn malformed_and_future_files_block_saving() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "version = [").unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.palettes, Palettes::builtin());
        assert!(loaded.templates.is_empty());
        assert!(loaded.save_blocked.unwrap().contains("line 1"));
        fs::write(&path, "version = 2\n[[template]]\nname = 'Future'\n").unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.templates.len(), 1);
        assert!(loaded.save_blocked.is_some());
        assert!(load(dir.path()).save_blocked.is_some());
        #[cfg(unix)]
        {
            let fifo = dir.path().join("fifo");
            assert!(
                std::process::Command::new("mkfifo")
                    .arg(&fifo)
                    .status()
                    .unwrap()
                    .success()
            );
            let loaded = load(&fifo);
            assert!(loaded.save_blocked.unwrap().contains("not a regular file"));
        }
    }

    #[test]
    fn config_path_precedence_does_not_depend_on_process_environment() {
        let path = |s: &str| Some(PathBuf::from(s));
        assert_eq!(
            path_from_env(path("flag"), path("env"), path("/xdg"), path("/home")),
            path("flag")
        );
        assert_eq!(
            path_from_env(None, path("env"), path("/xdg"), path("/home")),
            path("env")
        );
        assert_eq!(
            path_from_env(None, None, path("/xdg"), path("/home")),
            path("/xdg/rthid/config.toml")
        );
        assert_eq!(
            path_from_env(None, None, path("relative"), path("/home")),
            path("/home/.config/rthid/config.toml")
        );
        assert_eq!(
            path_from_env(None, None, None, path("/home")),
            path("/home/.config/rthid/config.toml")
        );
        assert_eq!(path_from_env(None, None, path("relative"), None), None);
    }
}
