//! User palettes and independent settings templates in a versioned TOML file.

use crate::settings::{Dither, Palettes, Preset, Settings, Template, Templates};
use rthid_dither::dither::{Mode, Tone};
use rthid_io::{
    atomic,
    io::{IoError, Job, Updated},
};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// Longest wait for the config lock during a background save.
const SAVE_WAIT: Duration = Duration::from_millis(50);
/// Delay before a failed save is tried again.
const SAVE_RETRY: Duration = Duration::from_secs(2);

// The edits carried by the newest save job.
struct Saving {
    /// Tag the result echoes.
    token: u64,
    /// Last edit sequence included in the job.
    through: u64,
}

/// Named configuration lists and their pending saves.
pub struct Library {
    /// Built in and user palette presets in selection order.
    pub palettes: Palettes,
    /// Saved settings with independent palette colors.
    pub templates: Templates,
    // Only the last unconfirmed edit for each list and name is retained.
    unsaved: Vec<(u64, Change)>,
    last_sequence: u64,
    // Whether edits arrived since the last save job.
    dirty: bool,
    saving: Option<Saving>,
    save_retry: Option<Instant>,
    save_blocked: Option<String>,
    save_error: Option<String>,
    load_notice: Option<String>,
}

impl Library {
    /// Starts with loaded lists and no pending edits.
    pub fn new(loaded: Loaded) -> Self {
        let load_notice = loaded.notice();
        Self {
            load_notice,
            palettes: loaded.palettes,
            templates: loaded.templates,
            unsaved: Vec::new(),
            last_sequence: 0,
            dirty: false,
            saving: None,
            save_retry: None,
            save_blocked: loaded.save_blocked,
            save_error: None,
        }
    }

    /// Unresolved save failure before load warnings and blocked saving for the status line.
    pub fn notice(&self) -> Option<&str> {
        self.save_error.as_deref().or(self.load_notice.as_deref())
    }

    /// Applies a validated edit and retains it until a save confirms it.
    pub fn commit(&mut self, change: Change) -> Result<(), String> {
        change.apply(&mut self.palettes, &mut self.templates)?;
        self.unsaved
            .retain(|(_, older)| older.key() != change.key());
        self.last_sequence += 1;
        self.unsaved.push((self.last_sequence, change));
        self.dirty = true;
        Ok(())
    }

    /// Edits not yet confirmed saved, empty when loading blocked writes.
    pub fn unsaved(&self) -> Vec<Change> {
        if self.save_blocked.is_some() {
            Vec::new()
        } else {
            self.unsaved
                .iter()
                .map(|(_, change)| change.clone())
                .collect()
        }
    }

    /// When a failed save is due to be tried again.
    pub fn retry_at(&self) -> Option<Instant> {
        self.save_retry
    }

    /// Creates a save when edits arrived or the retry deadline passed.
    pub fn save_job(&mut self, path: &Path, now: Instant, token: u64) -> Option<(Job, u64)> {
        if !self.dirty && self.save_retry.is_none_or(|deadline| now < deadline) {
            return None;
        }
        self.dirty = false;
        self.save_retry = None;
        let changes = self.unsaved();
        if changes.is_empty() {
            return None;
        }
        let job = Job::Update {
            path: path.to_path_buf(),
            wait: SAVE_WAIT,
            edit: Box::new(Merge(changes)),
        };
        self.saving = Some(Saving {
            token,
            through: self.last_sequence,
        });
        Some((job, token))
    }

    /// Applies a matching save result and returns None for stale tags.
    pub fn complete(
        &mut self,
        tag: u64,
        result: Result<Updated, IoError>,
        now: Instant,
    ) -> Option<()> {
        let saving = self.saving.take_if(|saving| saving.token == tag)?;
        match result {
            Ok(updated) => {
                // Later edits remain pending and visible over the saved lists.
                self.unsaved
                    .retain(|(sequence, _)| *sequence > saving.through);
                let mut loaded = parse(&updated.text);
                for (_, change) in &self.unsaved {
                    // A rejected edit remains for the next save to report.
                    let _ = change.apply(&mut loaded.palettes, &mut loaded.templates);
                }
                self.saved(loaded)
            }
            Err(error) => {
                self.save_retry = Some(now + SAVE_RETRY);
                self.save_failed(match &error.cause {
                    rthid_io::io::Cause::Io(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock =>
                    {
                        "file is busy".into()
                    }
                    cause => cause.to_string(),
                })
            }
        }
        Some(())
    }

    fn saved(&mut self, loaded: Loaded) {
        self.load_notice = loaded.notice();
        self.save_error = None;
        self.palettes = loaded.palettes;
        self.templates = loaded.templates;
        self.save_blocked = loaded.save_blocked;
    }

    fn save_failed(&mut self, error: String) {
        self.save_error = Some(format!("config: {error}"));
    }
}

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

    /// Applies this edit to in-memory lists with the rules used by the UI.
    pub fn apply(&self, palettes: &mut Palettes, templates: &mut Templates) -> Result<(), String> {
        match self {
            Self::UpsertPreset(preset) => palettes.upsert(preset.clone()).map(drop),
            Self::RemovePreset(name) => palettes.find(name).map_or(Ok(()), |i| palettes.remove(i)),
            Self::UpsertTemplate(template) => templates.upsert(template.clone()).map(drop),
            Self::RemoveTemplate(name) => {
                if let Some(index) = templates.find(name) {
                    templates.remove(index);
                }
                Ok(())
            }
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

/// Algorithm names stored in templates, kept apart from the app and library enums
/// so renaming a Rust variant cannot change the file format. An unknown name fails
/// only its own template entry, which is reported and kept raw on save rather than
/// read as some default algorithm.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum FileDither {
    Atkinson,
    FloydSteinberg,
    // The panel shows the short label Jarvis, but files store the full algorithm name.
    #[serde(rename = "jarvis-judice-ninke")]
    Jarvis,
    Stucki,
    SierraLite,
    Bayer2,
    Bayer4,
    Bayer8,
    Noise,
}
impl From<Dither> for FileDither {
    fn from(value: Dither) -> Self {
        match value {
            Dither::Atkinson => Self::Atkinson,
            Dither::FloydSteinberg => Self::FloydSteinberg,
            Dither::Jarvis => Self::Jarvis,
            Dither::Stucki => Self::Stucki,
            Dither::SierraLite => Self::SierraLite,
            Dither::Bayer2 => Self::Bayer2,
            Dither::Bayer4 => Self::Bayer4,
            Dither::Bayer8 => Self::Bayer8,
            Dither::Noise => Self::Noise,
        }
    }
}
impl From<FileDither> for Dither {
    fn from(value: FileDither) -> Self {
        match value {
            FileDither::Atkinson => Self::Atkinson,
            FileDither::FloydSteinberg => Self::FloydSteinberg,
            FileDither::Jarvis => Self::Jarvis,
            FileDither::Stucki => Self::Stucki,
            FileDither::SierraLite => Self::SierraLite,
            FileDither::Bayer2 => Self::Bayer2,
            FileDither::Bayer4 => Self::Bayer4,
            FileDither::Bayer8 => Self::Bayer8,
            FileDither::Noise => Self::Noise,
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
/// Missing fields take default settings, so older files without an algorithm or
/// seed load as Atkinson with seed 0.
#[derive(Serialize, Deserialize)]
#[serde(default)]
struct FileTemplate {
    name: String,
    palette: String,
    colors: Vec<String>,
    mode: FileMode,
    algorithm: FileDither,
    // Read as u64 to match the library range, then clamped to the app's u32 seed.
    seed: u64,
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
            algorithm: t.dither.into(),
            seed: u64::from(t.seed),
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
            dither: t.algorithm.into(),
            // Larger file seeds saturate at the app boundary instead of wrapping.
            seed: t.seed.min(u64::from(u32::MAX)) as u32,
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
    let text = match atomic::read_regular(path) {
        Ok(Some(text)) => text,
        Ok(None) => return Loaded::default(),
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
    match toml::from_str(text) {
        Ok(table) => parse_table(table),
        Err(error) => Loaded {
            save_blocked: Some(error.to_string()),
            ..Loaded::default()
        },
    }
}

fn parse_table(mut table: toml::Table) -> Loaded {
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
    let (toml::Value::Array(palettes), toml::Value::Array(templates)) = (palettes, templates)
    else {
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
        let entry: FilePreset = match value.try_into() {
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
        let entry: FileTemplate = match value.try_into() {
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
/// Waits up to `wait` for the config lock. TOML comments and formatting are lost on save.
pub fn save(path: &Path, wait: Duration, changes: &[Change]) -> io::Result<Loaded> {
    atomic::update(path, wait, |text| merge(text.as_deref(), changes))
}

/// Unsaved edits that a background Update job merges into the latest configuration text.
pub struct Merge(pub Vec<Change>);

impl rthid_io::io::Edit for Merge {
    fn apply(&self, text: Option<String>) -> io::Result<String> {
        merge(text.as_deref(), &self.0).map(|(text, _)| text)
    }
}

// Applies named edits to the current text, returning the new text and its parsed entries.
fn merge(text: Option<&str>, changes: &[Change]) -> io::Result<(String, Loaded)> {
    let mut table: toml::Table = text
        .filter(|text| !text.trim().is_empty())
        .map(toml::from_str)
        .transpose()
        .map_err(io::Error::other)?
        .unwrap_or_default();
    let mut loaded = parse_table(table.clone());
    if let Some(reason) = &loaded.save_blocked {
        return Err(io::Error::other(reason.clone()));
    }
    table.entry("version").or_insert(toml::Value::Integer(1));
    for change in changes {
        // Rejects edits the UI would reject before touching raw entries.
        change
            .apply(&mut loaded.palettes, &mut loaded.templates)
            .map_err(io::Error::other)?;
        let replacement = match change {
            Change::UpsertPreset(preset) => Some(toml::Value::try_from(FilePreset {
                name: preset.name.clone(),
                colors: preset.colors.clone(),
                mode: preset.mode.map(Into::into),
            })),
            Change::UpsertTemplate(template) => {
                Some(toml::Value::try_from(FileTemplate::from(template.clone())))
            }
            Change::RemovePreset(_) | Change::RemoveTemplate(_) => None,
        }
        .transpose()
        .map_err(io::Error::other)?;
        let (kind, name) = change.key();
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
    Ok((text, parse_table(table)))
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
    use std::fs;

    fn update(job: Job) -> (PathBuf, Box<dyn rthid_io::io::Edit>) {
        match job {
            Job::Update { path, edit, .. } => (path, edit),
            _ => panic!("save job expected"),
        }
    }

    fn template(name: &str) -> Change {
        Change::UpsertTemplate(Template::capture(name.into(), &Settings::default()))
    }

    #[test]
    fn algorithm_names_match_the_file_format() {
        let names = [
            "atkinson",
            "floyd-steinberg",
            "jarvis-judice-ninke",
            "stucki",
            "sierra-lite",
            "bayer2",
            "bayer4",
            "bayer8",
            "noise",
        ];
        for (dither, name) in Dither::ALL.into_iter().zip(names) {
            let value = toml::Value::try_from(FileDither::from(dither)).unwrap();
            assert_eq!(value.as_str(), Some(name));
        }
    }

    #[test]
    fn file_seed_is_clamped_at_the_domain_boundary() {
        let template = Template::from(FileTemplate {
            seed: u64::MAX,
            ..FileTemplate::default()
        });
        assert_eq!(template.seed, u32::MAX);
    }

    #[test]
    fn template_algorithms_default_and_round_trip() {
        let old = parse("version = 1\n[[template]]\nname = 'Old'\n");
        let old = old.templates.get(0).unwrap();
        assert_eq!(old.dither, Dither::Atkinson);
        assert_eq!(old.seed, 0);
        let settings = Settings {
            dither: Dither::Bayer4,
            seed: 7,
            ..Settings::default()
        };
        let template = Template::capture("Pattern".into(), &settings);
        let (text, loaded) = merge(None, &[Change::UpsertTemplate(template.clone())]).unwrap();
        assert!(text.contains("algorithm = \"bayer4\""));
        assert_eq!(loaded.templates.get(0), Some(&template));
    }

    #[test]
    fn failed_saves_keep_edits_and_retry_after_the_deadline() {
        use rthid_io::io::{Cause, IoError, Op, Updated};
        let config = Path::new("config.toml");
        let mut library = Library::new(Loaded::default());
        library.commit(template("Zine")).unwrap();
        let started = Instant::now();
        let (job, tag) = library.save_job(config, started, 1).unwrap();
        let (path, _) = update(job);
        let busy = IoError {
            op: Op::Save,
            path,
            cause: Cause::Io(std::io::Error::new(
                std::io::ErrorKind::WouldBlock,
                "config.toml: file is busy",
            )),
        };
        assert_eq!(library.complete(tag, Err(busy), started), Some(()));
        assert_eq!(library.notice(), Some("config: file is busy"));
        assert_eq!(library.unsaved().len(), 1);
        let deadline = library.retry_at().unwrap();
        assert_eq!(deadline, started + SAVE_RETRY);
        assert!(
            library
                .save_job(config, deadline - Duration::from_millis(1), 2)
                .is_none()
        );
        let (job, tag) = library.save_job(config, deadline, 2).unwrap();
        assert!(library.retry_at().is_none());
        let (path, edit) = update(job);
        let text = edit.apply(None).unwrap();
        assert_eq!(
            library.complete(tag, Ok(Updated { path, text }), Instant::now()),
            Some(())
        );
        assert!(library.unsaved().is_empty());
        assert!(library.templates.find("Zine").is_some());
    }

    #[test]
    fn equal_edit_made_during_a_save_remains_unsaved() {
        let now = Instant::now();
        let mut library = Library::new(Loaded::default());
        let change = template("T");
        library.commit(change.clone()).unwrap();
        let (job, tag) = library.save_job(Path::new("config.toml"), now, 1).unwrap();
        let (path, edit) = update(job);
        let text = edit.apply(None).unwrap();
        library.commit(change.clone()).unwrap();

        assert_eq!(
            library.complete(tag, Ok(Updated { path, text }), now),
            Some(())
        );
        assert_eq!(library.unsaved(), [change]);
    }

    #[test]
    fn edits_made_during_a_save_are_saved_after_it_succeeds() {
        use rthid_io::io::Updated;
        let config = Path::new("config.toml");
        let mut library = Library::new(Loaded::default());
        library.commit(template("First")).unwrap();
        let (job, tag) = library.save_job(config, Instant::now(), 1).unwrap();
        let (path, edit) = update(job);
        library.commit(template("Second")).unwrap();
        let text = edit.apply(None).unwrap();
        let first = || Updated {
            path: path.clone(),
            text: text.clone(),
        };
        assert!(
            library
                .complete(tag + 1, Ok(first()), Instant::now())
                .is_none()
        );
        assert_eq!(library.complete(tag, Ok(first()), Instant::now()), Some(()));
        assert_eq!(library.unsaved().len(), 1);
        assert!(library.templates.find("First").is_some());
        assert!(library.templates.find("Second").is_some());
        let (job, tag) = library.save_job(config, Instant::now(), 2).unwrap();
        let (path, edit) = update(job);
        let text = edit.apply(Some(text)).unwrap();
        assert_eq!(
            library.complete(tag, Ok(Updated { path, text }), Instant::now()),
            Some(())
        );
        assert!(library.unsaved().is_empty());
        let names: Vec<_> = library.templates.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["First", "Second"]);
        assert!(library.save_job(config, Instant::now(), 2).is_none());
    }

    #[test]
    fn failed_save_reports_repeated_errors() {
        let mut library = Library::new(Loaded::default());
        library.commit(template("First")).unwrap();
        library
            .commit(Change::RemoveTemplate("First".into()))
            .unwrap();
        assert_eq!(library.unsaved(), [Change::RemoveTemplate("First".into())]);
        library.save_failed("busy".into());
        assert_eq!(library.notice(), Some("config: busy"));
        library.save_failed("busy".into());
        assert_eq!(library.notice(), Some("config: busy"));
        library.saved(Loaded::default());
        assert!(library.notice().is_none());
        library.save_failed("denied".into());
        assert_eq!(library.notice(), Some("config: denied"));
        library.saved(Loaded::default());
        assert!(library.notice().is_none());
    }

    #[test]
    fn unsaved_edits_keep_the_last_change_per_list_and_name() {
        let mut library = Library::new(Loaded::default());
        let last = template("Shared");
        for change in [
            Change::RemovePreset("Shared".into()),
            Change::RemoveTemplate("Shared".into()),
            last.clone(),
            Change::RemovePreset("Other".into()),
            Change::RemovePreset("Other".into()),
        ] {
            library.commit(change).unwrap();
        }
        assert_eq!(
            library.unsaved(),
            [
                Change::RemovePreset("Shared".into()),
                last,
                Change::RemovePreset("Other".into())
            ]
        );
    }

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
        save(&path, Duration::from_secs(1), &changes).unwrap();
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
        let change = template("Saved");
        for text in ["", " \n\t", "[[template]]\nname = 'Existing'\n"] {
            fs::write(&path, text).unwrap();
            assert!(load(&path).save_blocked.is_none());
            let loaded =
                save(&path, Duration::from_secs(1), std::slice::from_ref(&change)).unwrap();
            assert!(loaded.templates.find("Saved").is_some());
            assert!(loaded.save_blocked.is_none());
            if !text.trim().is_empty() {
                assert!(loaded.templates.find("Existing").is_some());
            }
        }
    }

    #[test]
    fn background_merges_store_the_entries_a_direct_save_returns() {
        use rthid_io::io::Edit;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let changes = vec![template("Zine")];
        let existing = "[[template]]\nname = 'Existing'\n";
        let text = Merge(changes.clone()).apply(Some(existing.into())).unwrap();
        fs::write(&path, existing).unwrap();
        let saved = save(&path, Duration::from_secs(1), &changes).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), text);
        assert_eq!(parse(&text).templates, saved.templates);
        assert_eq!(parse(&text).palettes, saved.palettes);
        assert!(Merge(changes).apply(Some("version = 2".into())).is_err());
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
            first = save(&path, Duration::from_secs(1), &[upsert("First", "#000")]).unwrap();
            second = save(&path, Duration::from_secs(1), &[upsert("Second", "#fff")]).unwrap();
            let names = |loaded: &Loaded| -> Vec<String> {
                if presets {
                    loaded.palettes.user().map(|p| p.name.clone()).collect()
                } else {
                    loaded.templates.iter().map(|t| t.name.clone()).collect()
                }
            };
            assert_eq!(names(&first), ["First"]);
            assert_eq!(names(&second), ["First", "Second"]);
            save(
                &path,
                Duration::from_secs(1),
                &[remove("First"), remove("Missing")],
            )
            .unwrap();
            let merged = save(
                &path,
                Duration::from_secs(1),
                &[upsert("Second", "#123456")],
            )
            .unwrap();
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
        let original = "version = 1\nunknown = { enabled = true }\n[[palette]]\nname = 'Bad'\ncolors = 42\n[[palette]]\nname = 'Good'\ncolors = ['#fff']\n[[template]]\nname = 'Broken'\nalgorithm = 'future'\n";
        fs::write(&path, original).unwrap();
        let skipped = load(&path);
        assert!(skipped.templates.is_empty());
        assert!(
            skipped
                .warnings
                .iter()
                .any(|warning| warning.contains("future"))
        );
        let before: toml::Table = toml::from_str(original).unwrap();
        let loaded = save(&path, Duration::from_secs(1), &[template("New")]).unwrap();
        assert_eq!(loaded.warnings.len(), 2);
        let after: toml::Table = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(after["unknown"], before["unknown"]);
        assert_eq!(after["palette"], before["palette"]);
        assert_eq!(after["template"][0], before["template"][0]);
        save(
            &path,
            Duration::from_secs(1),
            &[Change::RemovePreset("Bad".into()), template("Broken")],
        )
        .unwrap();
        assert!(load(&path).warnings.is_empty());
    }

    #[test]
    fn save_rechecks_current_file_and_recovers_after_repair() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let change = template("Zine");
        save(&path, Duration::from_secs(1), &[]).unwrap();
        for text in ["version = [", "version = 2\n"] {
            fs::write(&path, text).unwrap();
            assert!(save(&path, Duration::from_secs(1), std::slice::from_ref(&change)).is_err());
            assert_eq!(fs::read(&path).unwrap(), text.as_bytes());
            fs::write(&path, "version = 1\n").unwrap();
            let loaded =
                save(&path, Duration::from_secs(1), std::slice::from_ref(&change)).unwrap();
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
