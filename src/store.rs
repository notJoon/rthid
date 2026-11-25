//! User palettes and independent settings templates in a versioned TOML file.

use crate::settings::{Palettes, Preset, Settings, Template, Templates};
use rthid_dither::{
    atomic,
    dither::{Mode, Tone},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

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

#[derive(Serialize, Deserialize)]
struct FileStore {
    version: u32,
    #[serde(default)]
    palette: Vec<FilePreset>,
    #[serde(default)]
    template: Vec<FileTemplate>,
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
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Loaded::default(),
        Err(error) => {
            return Loaded {
                save_blocked: Some(error.to_string()),
                ..Loaded::default()
            };
        }
    };
    let file: FileStore = match toml::from_str(&text) {
        Ok(file) => file,
        Err(error) => {
            return Loaded {
                save_blocked: Some(error.to_string()),
                ..Loaded::default()
            };
        }
    };
    let mut loaded = Loaded::default();
    if file.version > 1 {
        loaded.save_blocked = Some(format!("unsupported config version {}", file.version));
    }
    for entry in file.palette {
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
    for entry in file.template {
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

/// Serializes version 1 and atomically replaces the destination.
pub fn save(path: &Path, palettes: &Palettes, templates: &Templates) -> io::Result<()> {
    // TODO: Saving overwrites edits from other instances and drops comments, merge and preserve them.
    let file = FileStore {
        version: 1,
        palette: palettes
            .user()
            .map(|p| FilePreset {
                name: p.name.clone(),
                colors: p.colors.clone(),
                mode: p.mode.map(Into::into),
            })
            .collect(),
        template: templates.iter().cloned().map(Into::into).collect(),
    };
    let text = toml::to_string_pretty(&file).map_err(io::Error::other)?;
    atomic::replace(path, |file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
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
        save(&path, &palettes, &templates).unwrap();
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
