//! Versioned editing snapshots with a manifest committed after immutable pixel files.

use crate::{
    app::App,
    cells::View,
    document::{Document, ExposureMap, LUMA16_MIDPOINT, Layer},
    settings::{Rotation, Setting, Settings, Template, Templates, validate_name},
    store::FileTemplate,
    viewport::{Viewport, Zoom},
};
use image::{DynamicImage, ImageBuffer, ImageFormat, Luma, RgbaImage};
use rthid_dither::{dither::PaletteColor, mass::MassOptions};
use rthid_io::atomic;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

/// View controls stored independently of brush and undo state.
#[derive(Clone, PartialEq)]
pub(crate) struct ViewState {
    view: View,
    mass_return: View,
    mass_minimap: bool,
    mass: MassOptions,
    mass_tones: Option<[PaletteColor; 4]>,
    viewport: Viewport,
}

impl ViewState {
    /// Captures the user controlled preview state.
    pub(crate) fn capture(app: &App) -> Self {
        Self {
            view: app.view,
            mass_return: app.mass_return,
            mass_minimap: app.mass_minimap,
            mass: app.mass.clone(),
            mass_tones: app.mass_tones,
            viewport: app.viewport,
        }
    }

    /// Captures document preview choices without terminal viewport preferences.
    pub(crate) fn capture_unsaved(app: &App) -> Self {
        let mut state = Self::capture(app);
        state.viewport = Viewport::default();
        state
    }

    /// Restores preview controls without changing brush controls.
    pub(crate) fn restore(self, app: &mut App) {
        app.view = self.view;
        app.mass_return = self.mass_return;
        app.mass_minimap = self.mass_minimap;
        app.mass = self.mass;
        app.mass_tones = self.mass_tones;
        app.viewport = self.viewport;
    }
}

/// Owned session data transferred to and from the filesystem worker.
pub(crate) struct Session {
    /// Shrunk immutable source pixels.
    pub source: RgbaImage,
    /// Source location set by `read` and ignored by `write`.
    pub source_path: PathBuf,
    /// Layer order, selection and exposure offsets.
    pub document: Document,
    /// Complete processing settings.
    pub settings: Settings,
    /// User controlled preview state.
    pub view: ViewState,
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    version: u32,
    source: String,
    source_width: u32,
    source_height: u32,
    source_hash: String,
    settings: FileSettings,
    view: FileView,
    layers: Vec<FileLayer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<usize>,
}
#[derive(Serialize, Deserialize)]
struct FileSettings {
    #[serde(flatten)]
    template: FileTemplate,
    zoom: i32,
    pan_x: i32,
    pan_y: i32,
    rotation: u16,
    flip: bool,
}
#[derive(Serialize, Deserialize)]
struct FileLayer {
    name: String,
    visible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    exposure: Option<String>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum FileViewKind {
    Original,
    Dithered,
    Mass,
}
impl From<View> for FileViewKind {
    fn from(view: View) -> Self {
        match view {
            View::Original => Self::Original,
            View::Dithered => Self::Dithered,
            View::Mass => Self::Mass,
        }
    }
}
impl From<FileViewKind> for View {
    fn from(view: FileViewKind) -> Self {
        match view {
            FileViewKind::Original => Self::Original,
            FileViewKind::Dithered => Self::Dithered,
            FileViewKind::Mass => Self::Mass,
        }
    }
}
#[derive(Serialize, Deserialize)]
struct FileView {
    view: FileViewKind,
    mass_return: FileViewKind,
    mass_minimap: bool,
    sigma: f32,
    thresholds: [f32; 3],
    invert: bool,
    ellipses: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    mass_tones: Option<[[u8; 3]; 4]>,
    zoom: i8,
    x: u32,
    y: u32,
    hide_minimap: bool,
}
impl From<&ViewState> for FileView {
    fn from(v: &ViewState) -> Self {
        Self {
            view: v.view.into(),
            mass_return: v.mass_return.into(),
            mass_minimap: v.mass_minimap,
            sigma: v.mass.sigma,
            thresholds: v.mass.thresholds,
            invert: v.mass.invert,
            ellipses: v.mass.ellipses,
            mass_tones: v.mass_tones.map(|tones| tones.map(|tone| tone.rgb)),
            zoom: match v.viewport.zoom {
                Zoom::Fit => 0,
                Zoom::In(n) => n as i8,
                Zoom::Out(n) => -(n as i8),
            },
            x: v.viewport.x,
            y: v.viewport.y,
            hide_minimap: v.viewport.hide_minimap,
        }
    }
}
impl FileView {
    fn into_state(self) -> io::Result<ViewState> {
        if !self.sigma.is_finite()
            || self.sigma < 0.0
            || self
                .thresholds
                .iter()
                .any(|t| !t.is_finite() || *t <= 0.0 || *t > 1.0)
            || self.thresholds[0] >= self.thresholds[1]
            || self.thresholds[1] >= self.thresholds[2]
        {
            return Err(io::Error::other("invalid mass options"));
        }
        let zoom = match self.zoom {
            0 => Zoom::Fit,
            1..=4 => Zoom::In(self.zoom as u8),
            -4..=-2 => Zoom::Out((-self.zoom) as u8),
            _ => return Err(io::Error::other("invalid viewport zoom")),
        };
        Ok(ViewState {
            view: self.view.into(),
            mass_return: self.mass_return.into(),
            mass_minimap: self.mass_minimap,
            mass: MassOptions {
                sigma: self.sigma,
                thresholds: self.thresholds,
                invert: self.invert,
                ellipses: self.ellipses,
            },
            mass_tones: self
                .mass_tones
                .map(|tones| tones.map(|rgb| PaletteColor { rgb })),
            viewport: Viewport {
                zoom,
                x: self.x,
                y: self.y,
                hide_minimap: self.hide_minimap,
            },
        })
    }
}

fn named(path: &Path, error: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("{}: {error}", path.display()))
}

/// Recognizes session directories without teaching the IO crate their format.
pub(crate) fn is_session(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "rthid")
        && path.is_dir()
        && path.join("manifest.toml").is_file()
}

/// Recognizes a directory a save may write: a session, or what a failed or
/// interrupted first save left before its manifest.
pub(crate) fn is_saveable(path: &Path) -> bool {
    is_session(path)
        || path.extension().is_some_and(|ext| ext == "rthid")
            && only_save_leftovers(path).unwrap_or(false)
}

/// Whether `dir` would sit directly inside a session or a save's leftovers.
pub(crate) fn nested(dir: &Path) -> bool {
    resolve(dir)
        .ok()
        .and_then(|dir| dir.parent().map(is_saveable))
        .unwrap_or(false)
}

/// Whether `dir` holds only the lock, generation files, and `atomic` temporaries.
fn only_save_leftovers(dir: &Path) -> io::Result<bool> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".lock" {
            continue;
        }
        let owned = name.to_str().is_some_and(|name| {
            generation_number(name).is_some()
                || name.starts_with(".rthid-") && name.ends_with(".tmp")
        });
        if !(owned && entry.file_type()?.is_file()) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Uses the source stem beside the original image.
pub(crate) fn default_path(source: &Path) -> PathBuf {
    source.with_extension("rthid")
}

fn child(dir: &Path, name: &str) -> io::Result<PathBuf> {
    if Path::new(name).components().count() != 1
        || !matches!(
            Path::new(name).components().next(),
            Some(std::path::Component::Normal(_))
        )
    {
        return Err(named(&dir.join("manifest.toml"), "invalid pixel file name"));
    }
    Ok(dir.join(name))
}

fn parse_manifest(path: &Path, text: &str) -> io::Result<Manifest> {
    let broken = |error: &dyn std::fmt::Display| {
        named(
            path,
            format!("broken manifest, restore it or save elsewhere: {error}"),
        )
    };
    let value: toml::Value = toml::from_str(text).map_err(|e| broken(&e))?;
    let version = value
        .get("version")
        .and_then(toml::Value::as_integer)
        .ok_or_else(|| broken(&"missing or invalid version"))?;
    if version != 1 {
        let reason = if version > 1 {
            "was written by a newer rthid"
        } else {
            "has an unsupported session version"
        };
        return Err(named(
            path,
            format!("{reason} (version {version}), upgrade rthid or save elsewhere"),
        ));
    }
    toml::from_str(text).map_err(|e| broken(&e))
}

fn resolve(path: &Path) -> io::Result<PathBuf> {
    let mut ancestor = path;
    loop {
        let existing = if ancestor.as_os_str().is_empty() {
            Path::new(".")
        } else {
            ancestor
        };
        match existing.canonicalize() {
            Ok(resolved) => return Ok(resolved.join(path.strip_prefix(ancestor).unwrap())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                ancestor = ancestor.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Rejects exports whose resolved destination is inside the session.
pub(crate) fn check_exports(dir: &Path, exports: &[impl AsRef<Path>]) -> io::Result<()> {
    let dir = resolve(dir)?;
    for export in exports {
        let path = export.as_ref();
        if resolve(path)?.starts_with(&dir) {
            return Err(named(
                path,
                "export destination is inside the session directory",
            ));
        }
    }
    Ok(())
}

/// Validates the manifest and pixel files before returning a complete session.
pub(crate) fn read(dir: &Path) -> io::Result<Session> {
    read_with(dir, || {})
}

fn read_with(dir: &Path, mut after_manifest: impl FnMut()) -> io::Result<Session> {
    for attempt in 0..3 {
        match read_once(dir, &mut after_manifest) {
            Err(error) if error.kind() == io::ErrorKind::NotFound && attempt < 2 => continue,
            result => return result,
        }
    }
    unreachable!()
}

fn pixel_error(path: &Path, error: io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

fn reject_symlink(path: &Path) -> io::Result<()> {
    if fs::symlink_metadata(path)
        .map_err(|error| pixel_error(path, error))?
        .file_type()
        .is_symlink()
    {
        return Err(named(path, "session files must not be symlinks"));
    }
    Ok(())
}

fn read_once(dir: &Path, after_manifest: &mut impl FnMut()) -> io::Result<Session> {
    let path = dir.join("manifest.toml");
    let text = atomic::read_regular(&path)
        .map_err(|e| named(&path, e))?
        .ok_or_else(|| named(&path, "missing manifest"))?;
    let manifest = parse_manifest(&path, &text)?;
    after_manifest();
    if manifest.layers.len() > 256 {
        return Err(named(&path, "at most 256 layers are allowed"));
    }
    let source_path = child(dir, &manifest.source)?;
    reject_symlink(&source_path)?;
    let source = rthid_io::image::load_image(&source_path).map_err(|error| match error {
        image::ImageError::IoError(error) => pixel_error(&source_path, error),
        error => named(&source_path, error),
    })?;
    let (width, height) = source.dimensions();
    let mut layers = Vec::new();
    if (width, height) != (manifest.source_width, manifest.source_height)
        || source_hash(&source) != manifest.source_hash
    {
        return Err(named(
            &source_path,
            "source pixels differ from manifest, the session copy was modified outside rthid, reopen the original image",
        ));
    }
    for layer in manifest.layers {
        validate_name(&layer.name, "layer").map_err(|e| named(&path, e))?;
        if layer.name.chars().any(char::is_control) {
            return Err(named(&path, "layer name contains control characters"));
        }
        let exposure = if let Some(name) = layer.exposure {
            let layer_path = child(dir, &name)?;
            reject_symlink(&layer_path)?;
            let mut reader = image::ImageReader::with_format(
                io::BufReader::new(
                    rthid_io::image::open_regular(&layer_path)
                        .map_err(|e| pixel_error(&layer_path, e))?,
                ),
                ImageFormat::Png,
            );
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(width);
            limits.max_image_height = Some(height);
            limits.max_alloc = Some(256 * 1024 * 1024);
            reader.limits(limits);
            let image = reader.decode().map_err(|e| named(&layer_path, e))?;
            let DynamicImage::ImageLuma16(image) = image else {
                return Err(named(&layer_path, "expected 16 bit grayscale PNG"));
            };
            if image.dimensions() != (width, height) {
                return Err(named(&layer_path, "layer dimensions differ from source"));
            }
            ExposureMap::from_luma16(width, height, image.as_raw())
        } else {
            ExposureMap::new(width, height)
        };
        let mut restored = Layer::new(layer.name, width, height);
        restored.visible = layer.visible;
        restored.exposure = exposure;
        layers.push(restored);
    }
    let document = Document::from_layers(width, height, layers, manifest.selected)
        .map_err(|e| named(&path, e))?;
    let file = manifest.settings;
    let mut template: Template = file.template.into();
    template.name = "Session".into();
    let mut templates = Templates::default();
    templates.upsert(template).map_err(|e| named(&path, e))?;
    let mut settings = templates
        .get(0)
        .unwrap()
        .apply(&Settings::default())
        .map_err(|e| named(&path, e))?;
    settings.zoom = Setting::Zoom.snap(f64::from(file.zoom)) as i32;
    settings.pan_x = Setting::PanX.snap(f64::from(file.pan_x)) as i32;
    settings.pan_y = Setting::PanY.snap(f64::from(file.pan_y)) as i32;
    settings.rotation = match file.rotation {
        0 => Rotation::R0,
        90 => Rotation::R90,
        180 => Rotation::R180,
        270 => Rotation::R270,
        _ => return Err(named(&path, "invalid rotation")),
    };
    settings.flip = file.flip;
    settings
        .dimensions((width, height))
        .map_err(|e| named(&path, e))?;
    let view = manifest.view.into_state().map_err(|e| named(&path, e))?;
    Ok(Session {
        source_path,
        source,
        document,
        settings,
        view,
    })
}

fn source_hash(source: &RgbaImage) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in source.as_raw() {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn generation_number(name: &str) -> Option<u64> {
    let number = if let Some(suffix) = name.strip_prefix("source-") {
        suffix.strip_suffix(".png")?.parse::<u64>().ok()?
    } else {
        let (number, index) = name
            .strip_prefix("exposure-")?
            .strip_suffix(".png")?
            .split_once('-')?;
        index.parse::<usize>().ok()?;
        number.parse::<u64>().ok()?
    };
    Some(number)
}

/// Routes a file inside a session directory back to that session.
pub(crate) fn containing(path: &Path) -> Option<PathBuf> {
    let path = path.canonicalize().ok()?;
    if !path.is_file() {
        return None;
    }
    let dir = path.parent()?;
    is_session(dir).then(|| dir.to_path_buf())
}

/// Writes the session, creating the directory if necessary.
/// Commits pixel files before the manifest and removes unreferenced
/// source and exposure files with valid generation names.
///
/// # Errors
///
/// Returns an error if the directory has no manifest and contains
/// files other than recognized generation files, or if the existing
/// manifest is invalid or unsupported.
pub(crate) fn write(dir: &Path, session: Session) -> io::Result<()> {
    write_before_manifest(dir, session, || Ok(()))
}

fn write_before_manifest(
    dir: &Path,
    session: Session,
    before_manifest: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    write_with_sync(dir, session, before_manifest, atomic::sync_dir)
}

fn create_session_dir(
    dir: &Path,
    sync: &mut impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    if dir.is_dir() {
        return Ok(());
    }
    let parent = dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_session_dir(parent, sync)?;
    match fs::create_dir(dir) {
        Ok(()) => sync(parent),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
        Err(error) => Err(error),
    }
}

fn write_with_sync(
    dir: &Path,
    session: Session,
    before_manifest: impl FnOnce() -> io::Result<()>,
    mut sync: impl FnMut(&Path) -> io::Result<()>,
) -> io::Result<()> {
    if session.document.layers().len() > 256 {
        return Err(io::Error::other("at most 256 layers are allowed"));
    }
    create_session_dir(dir, &mut sync)?;
    let _lock =
        atomic::lock(&dir.join(".lock"), std::time::Duration::from_secs(2)).map_err(|error| {
            if error.kind() == io::ErrorKind::WouldBlock {
                io::Error::new(
                    error.kind(),
                    "another rthid is saving this session, try again",
                )
            } else {
                error
            }
        })?;
    let (width, height) = session.source.dimensions();
    let hash = source_hash(&session.source);
    let manifest_path = dir.join("manifest.toml");
    if fs::symlink_metadata(&manifest_path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(named(&manifest_path, "session files must not be symlinks"));
    }
    let previous = atomic::read_regular(&manifest_path)?
        .map(|text| parse_manifest(&manifest_path, &text))
        .transpose()?;
    // Leftovers of a failed first save are reused, and the cleanup below removes them.
    if previous.is_none() && !only_save_leftovers(dir)? {
        return Err(named(dir, "not a session directory, save elsewhere"));
    }
    let mut generation = 0u64;
    let names = previous.iter().flat_map(|m| {
        std::iter::once(m.source.clone()).chain(m.layers.iter().filter_map(|l| l.exposure.clone()))
    });
    for value in
        names
            .filter_map(|name| generation_number(&name))
            .chain(fs::read_dir(dir)?.filter_map(|entry| {
                entry
                    .ok()
                    .and_then(|entry| generation_number(&entry.file_name().to_string_lossy()))
                    .filter(|value| *value <= u64::from(u32::MAX))
            }))
    {
        generation = generation.max(value);
    }
    let entries = fs::read_dir(dir)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<io::Result<std::collections::HashSet<_>>>()?;
    let generation = loop {
        generation = generation
            .checked_add(1)
            .ok_or_else(|| io::Error::other("session generation exhausted"))?;
        if !entries.contains(std::ffi::OsStr::new(&format!("source-{generation}.png")))
            && (0..session.document.layers().len()).all(|index| {
                !entries.contains(std::ffi::OsStr::new(&format!(
                    "exposure-{generation}-{index}.png"
                )))
            })
        {
            break generation;
        }
    };
    let reusable = previous.filter(|m| {
        m.source_width == width
            && m.source_height == height
            && m.source_hash == hash
            && child(dir, &m.source).is_ok_and(|path| {
                fs::symlink_metadata(&path).is_ok_and(|m| m.is_file())
                    && rthid_io::image::load_image(&path)
                        .is_ok_and(|pixels| pixels == session.source)
            })
    });
    let source = if let Some(previous) = reusable {
        previous.source
    } else {
        let name = format!("source-{generation}.png");
        atomic::replace(&dir.join(&name), |file| {
            DynamicImage::ImageRgba8(session.source)
                .write_to(&mut *file, ImageFormat::Png)
                .map_err(io::Error::other)?;
            file.sync_all()
        })?;
        name
    };
    let mut layers = Vec::new();
    for (index, layer) in session.document.layers().iter().enumerate() {
        let pixels = layer.exposure.to_luma16();
        let exposure = if pixels.iter().any(|value| *value != LUMA16_MIDPOINT) {
            let name = format!("exposure-{generation}-{index}.png");
            let image: ImageBuffer<Luma<u16>, Vec<u16>> =
                ImageBuffer::from_raw(width, height, pixels)
                    .ok_or_else(|| io::Error::other("invalid exposure dimensions"))?;
            atomic::replace(&dir.join(&name), |file| {
                DynamicImage::ImageLuma16(image)
                    .write_to(&mut *file, ImageFormat::Png)
                    .map_err(io::Error::other)?;
                file.sync_all()
            })?;
            Some(name)
        } else {
            None
        };
        layers.push(FileLayer {
            name: layer.name.clone(),
            visible: layer.visible,
            exposure,
        });
    }
    let settings = session.settings;
    let manifest = Manifest {
        version: 1,
        source,
        source_width: width,
        source_height: height,
        source_hash: hash,
        settings: FileSettings {
            template: Template::capture("Session".into(), &settings).into(),
            zoom: settings.zoom,
            pan_x: settings.pan_x,
            pan_y: settings.pan_y,
            rotation: settings.rotation.degrees(),
            flip: settings.flip,
        },
        view: (&session.view).into(),
        layers,
        selected: session.document.selected(),
    };
    let text = toml::to_string_pretty(&manifest).map_err(io::Error::other)?;
    before_manifest()?;
    sync(dir)?;
    atomic::replace(&dir.join("manifest.toml"), |file| {
        file.write_all(text.as_bytes())?;
        file.sync_all()
    })?;
    // Keep previous pixels until the manifest entry is durable.
    if sync(dir).is_ok() {
        cleanup(dir, &manifest);
    }
    Ok(())
}

fn cleanup(dir: &Path, manifest: &Manifest) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if generation_number(&name).is_some()
            && name != manifest.source
            && !manifest
                .layers
                .iter()
                .any(|layer| layer.exposure.as_deref() == Some(name.as_ref()))
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{BUSINESS_CARD, Dither, Palette};
    use image::{GrayImage, Rgba};

    #[test]
    fn check_exports_handles_missing_session_directory() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("missing/photo.rthid");
        for name in ["preview.png", "nested/preview.png"] {
            let error = check_exports(&dir, &[dir.join(name)]).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("export destination is inside the session directory")
            );
        }
        assert!(check_exports(&dir, &[dir.with_file_name("preview.png")]).is_ok());
        assert!(!dir.exists());
    }

    fn fixture() -> Session {
        let mut app = App::builtin(RgbaImage::from_pixel(67, 65, Rgba([23, 64, 96, 200])), None);
        app.document.add_layer("Bottom".into());
        let map = &mut app.document.target_mut().unwrap().exposure;
        map.add(0, 0, i16::MIN);
        map.add(66, 64, i16::MAX);
        map.add(64, 0, -512);
        app.document.add_layer("Empty".into());
        app.document.add_layer("Hidden".into());
        app.document.target_mut().unwrap().exposure.add(1, 1, 256);
        app.document.set_visible(2, false);
        app.document.select(Some(0));
        app.document.commit_stroke(0, ExposureMap::new(67, 65));
        app.settings.card = BUSINESS_CARD;
        app.settings.dot = 25;
        app.settings.margin = 10;
        app.settings.bleed = 3;
        app.settings.zoom = 120;
        app.settings.pan_x = -25;
        app.settings.pan_y = 45;
        app.settings.rotation = Rotation::R270;
        app.settings.flip = true;
        app.settings.seed = u32::MAX;
        app.settings.dither = Dither::Noise;
        app.settings.strength = 0.75;
        app.settings.serpentine = false;
        app.settings.palette =
            Palette::new("Custom".into(), vec!["#123456".into(), "#fedcba".into()]).unwrap();
        app.settings.tone.brightness = -0.2;
        app.settings.tone.contrast = 0.3;
        app.settings.tone.gamma = 1.4;
        app.settings.tone.black = 0.1;
        app.settings.tone.white = 0.8;
        app.settings.tone.sharpen = 0.5;
        app.view = View::Mass;
        app.mass_return = View::Dithered;
        app.mass_minimap = false;
        app.mass.sigma = 2.5;
        app.mass.thresholds = [0.2, 0.6, 0.9];
        app.mass.invert = true;
        app.mass.ellipses = false;
        app.mass_tones = Some(rthid_dither::mass::WARM);
        app.viewport = Viewport {
            zoom: Zoom::Out(3),
            x: 11,
            y: 21,
            hide_minimap: true,
        };
        Session {
            source_path: PathBuf::new(),
            source: app.grid_cache.source.clone().unwrap(),
            document: app.document.clone(),
            settings: app.settings.clone(),
            view: ViewState::capture(&app),
        }
    }

    fn assert_equal(a: &Session, b: &Session) {
        assert_eq!(a.source, b.source);
        crate::document::assert_same_content(&a.document, &b.document);
        assert!(a.settings == b.settings);
        assert!(a.view == b.view);
    }

    fn exposure_files(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with("exposure-"))
            .collect();
        names.sort();
        names
    }

    #[test]
    fn read_retries_only_missing_pixels_with_a_fresh_manifest() {
        for missing_source in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            write(temp.path(), fixture()).unwrap();
            let mut attempts = 0;
            let loaded = read_with(temp.path(), || {
                attempts += 1;
                if attempts == 1 {
                    let mut next = fixture();
                    if missing_source {
                        next.source.put_pixel(0, 0, Rgba([1; 4]));
                    }
                    write(temp.path(), next).unwrap();
                }
            })
            .unwrap();
            assert_eq!(attempts, 2);
            assert_eq!(
                loaded.source.get_pixel(0, 0),
                if missing_source {
                    &Rgba([1; 4])
                } else {
                    &Rgba([23, 64, 96, 200])
                }
            );
        }
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        fs::remove_file(temp.path().join("source-1.png")).unwrap();
        let mut attempts = 0;
        assert!(
            read_with(temp.path(), || {
                attempts += 1;
            })
            .is_err()
        );
        assert_eq!(attempts, 3);
        fs::write(temp.path().join("source-1.png"), b"broken").unwrap();
        attempts = 0;
        assert!(
            read_with(temp.path(), || {
                attempts += 1;
            })
            .is_err()
        );
        assert_eq!(attempts, 1);
    }

    #[test]
    fn first_save_syncs_each_created_directory_parent_and_propagates_failure() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("parent/photo.rthid");
        let mut synced = Vec::new();
        write_with_sync(
            &dir,
            fixture(),
            || Ok(()),
            |path| {
                synced.push(path.to_path_buf());
                atomic::sync_dir(path)
            },
        )
        .unwrap();
        assert_eq!(synced, [temp.path(), dir.parent().unwrap(), &dir, &dir]);
        synced.clear();
        write_with_sync(
            &dir,
            fixture(),
            || Ok(()),
            |path| {
                synced.push(path.to_path_buf());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(synced, [dir.as_path(), dir.as_path()]);
        let failed = temp.path().join("failed/photo.rthid");
        let error = write_with_sync(
            &failed,
            fixture(),
            || Ok(()),
            |_| Err(io::Error::other("parent sync failed")),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "parent sync failed");
        assert!(!failed.join("manifest.toml").exists());
    }

    #[test]
    #[cfg(unix)]
    fn generation_collision_never_follows_existing_pixel_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        let outside = temp.path().join("outside");
        fs::write(&outside, b"keep").unwrap();
        fs::write(temp.path().join("source-4294967295.png"), b"stray").unwrap();
        let link = temp.path().join("exposure-4294967296-0.png");
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let manifest_path = temp.path().join("manifest.toml");
        let mut manifest: Manifest =
            toml::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
        manifest.layers[0].exposure = Some("exposure-4294967295-0.png".into());
        fs::write(manifest_path, toml::to_string(&manifest).unwrap()).unwrap();
        write(temp.path(), fixture()).unwrap();
        assert_eq!(fs::read(outside).unwrap(), b"keep");
        assert!(temp.path().join("exposure-4294967297-0.png").exists());
        assert_equal(&fixture(), &read(temp.path()).unwrap());
    }

    #[test]
    fn failed_manifest_directory_sync_preserves_both_generations_until_next_save() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        let previous = fs::read(temp.path().join("manifest.toml")).unwrap();
        let mut changed = fixture();
        changed.source.put_pixel(0, 0, Rgba([1; 4]));
        let mut calls = 0;
        write_with_sync(
            temp.path(),
            changed,
            || Ok(()),
            |_| {
                calls += 1;
                if calls == 2 {
                    Err(io::Error::other("manifest sync failed"))
                } else {
                    Ok(())
                }
            },
        )
        .unwrap();
        assert_eq!(calls, 2);
        assert!(temp.path().join("source-1.png").exists());
        assert!(temp.path().join("exposure-1-0.png").exists());
        assert!(temp.path().join("source-2.png").exists());
        assert_eq!(
            read(temp.path()).unwrap().source.get_pixel(0, 0),
            &Rgba([1; 4])
        );
        fs::write(temp.path().join("manifest.toml"), previous).unwrap();
        assert_equal(&fixture(), &read(temp.path()).unwrap());
        write(temp.path(), fixture()).unwrap();
        assert!(!temp.path().join("exposure-1-0.png").exists());
        assert!(!temp.path().join("source-2.png").exists());
    }

    #[test]
    fn refuses_non_session_directories_and_preserves_unrecognized_names() {
        let temp = tempfile::tempdir().unwrap();
        let scan = temp.path().join("source-scan.png");
        fs::write(&scan, b"keep").unwrap();
        assert!(
            write(temp.path(), fixture())
                .unwrap_err()
                .to_string()
                .contains("not a session directory")
        );
        assert_eq!(fs::read(&scan).unwrap(), b"keep");
        fs::remove_file(&scan).unwrap();
        write(temp.path(), fixture()).unwrap();
        fs::write(&scan, b"keep").unwrap();
        write(temp.path(), fixture()).unwrap();
        assert_eq!(fs::read(scan).unwrap(), b"keep");
    }

    #[test]
    fn save_refuses_newer_and_broken_manifests_without_changing_files() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        let path = temp.path().join("manifest.toml");
        let mut names: Vec<_> = fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        for (text, message) in [
            ("version = 2", "newer rthid (version 2), upgrade rthid"),
            ("[broken", "broken manifest"),
            ("version = 1", "broken manifest"),
        ] {
            fs::write(&path, text).unwrap();
            let error = write(temp.path(), fixture()).unwrap_err().to_string();
            assert!(
                error.contains("manifest.toml") && error.contains(message),
                "{error}"
            );
            let read_error = read(temp.path()).err().unwrap().to_string();
            assert!(read_error.contains(message), "{read_error}");
            assert_eq!(fs::read_to_string(&path).unwrap(), text);
            let mut after: Vec<_> = fs::read_dir(temp.path())
                .unwrap()
                .map(|entry| entry.unwrap().file_name())
                .collect();
            after.sort();
            assert_eq!(after, names);
        }
    }

    #[test]
    fn lock_timeout_tells_the_user_to_retry() {
        let temp = tempfile::tempdir().unwrap();
        let _lock = atomic::lock(&temp.path().join(".lock"), std::time::Duration::ZERO).unwrap();
        let error = write(temp.path(), fixture()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
        assert_eq!(
            error.to_string(),
            "another rthid is saving this session, try again"
        );
    }

    #[test]
    fn changed_source_pixels_tell_the_user_to_reopen_the_original() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        let path = read(temp.path()).unwrap().source_path;
        let mut source = fixture().source;
        source.put_pixel(0, 0, Rgba([1; 4]));
        source.save(path).unwrap();
        let error = read(temp.path()).err().unwrap().to_string();
        assert!(error.contains("session copy was modified outside rthid"));
        assert!(error.contains("reopen the original image"));
    }

    #[test]
    fn replacing_source_and_repairing_corruption_uses_new_files() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        let mut changed = fixture();
        changed.source.put_pixel(0, 0, Rgba([9, 8, 7, 6]));
        let expected = changed.source.clone();
        write(temp.path(), changed).unwrap();
        let loaded = read(temp.path()).unwrap();
        assert_eq!(loaded.source, expected);
        assert!(loaded.source_path.ends_with("source-2.png"));
        assert!(!temp.path().join("source-1.png").exists());
        fs::write(&loaded.source_path, b"broken").unwrap();
        let mut changed = fixture();
        changed.source = expected.clone();
        write(temp.path(), changed).unwrap();
        let loaded = read(temp.path()).unwrap();
        assert_eq!(loaded.source, expected);
        assert!(loaded.source_path.ends_with("source-3.png"));
        assert_eq!(source_hash(&RgbaImage::new(0, 0)), "cbf29ce484222325");
        assert_eq!(
            source_hash(&RgbaImage::from_pixel(1, 1, Rgba([0; 4]))),
            "4d25767f9dce13f5"
        );
    }

    #[test]
    fn cleanup_failures_leave_a_committed_save_successful() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        let orphan = temp.path().join("source-99.png");
        fs::create_dir(&orphan).unwrap();
        fs::write(orphan.join("keep"), b"cannot delete as a file").unwrap();
        write(temp.path(), fixture()).unwrap();
        assert_equal(&fixture(), &read(temp.path()).unwrap());
        let manifest: Manifest =
            toml::from_str(&fs::read_to_string(temp.path().join("manifest.toml")).unwrap())
                .unwrap();
        cleanup(&temp.path().join("missing"), &manifest);
        cleanup(&temp.path().join("manifest.toml"), &manifest);
    }

    #[test]
    fn concurrent_writers_hold_the_lock_through_commit() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().to_path_buf();
        let (entered, waiting) = std::sync::mpsc::channel();
        let (release, resume) = std::sync::mpsc::channel();
        let first = dir.clone();
        let writer = std::thread::spawn(move || {
            write_before_manifest(&first, fixture(), || {
                entered.send(()).unwrap();
                resume.recv().unwrap();
                Ok(())
            })
            .unwrap();
        });
        waiting
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let lock = fs::File::open(dir.join(".lock")).unwrap();
        assert!(matches!(lock.try_lock(), Err(fs::TryLockError::WouldBlock)));
        let second = dir.clone();
        let writer2 = std::thread::spawn(move || {
            let mut snapshot = fixture();
            snapshot.source.put_pixel(0, 0, Rgba([1; 4]));
            write(&second, snapshot).unwrap();
        });
        release.send(()).unwrap();
        writer.join().unwrap();
        writer2.join().unwrap();
        assert_eq!(read(&dir).unwrap().source.get_pixel(0, 0), &Rgba([1; 4]));
        assert!(dir.join(".lock").exists());
    }

    #[test]
    fn rejects_invalid_layer_names_and_too_many_layers() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        let path = temp.path().join("manifest.toml");
        let text = fs::read_to_string(&path).unwrap();
        for name in [
            "".to_owned(),
            " ".into(),
            "x".repeat(33),
            "a\nb".into(),
            "a\u{7f}b".into(),
        ] {
            let mut manifest: Manifest = toml::from_str(&text).unwrap();
            manifest.layers[0].name = name;
            fs::write(&path, toml::to_string(&manifest).unwrap()).unwrap();
            assert!(
                read(temp.path())
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("layer")
            );
        }
        let mut oversized = fixture();
        for _ in 0..257 {
            oversized.document.add_layer("Layer".into());
        }
        assert!(
            write(temp.path(), oversized)
                .unwrap_err()
                .to_string()
                .contains("256")
        );
        let mut manifest: Manifest = toml::from_str(&text).unwrap();
        manifest.layers = (0..257)
            .map(|_| FileLayer {
                name: "Layer".into(),
                visible: true,
                exposure: None,
            })
            .collect();
        fs::write(&path, toml::to_string(&manifest).unwrap()).unwrap();
        assert!(read(temp.path()).err().unwrap().to_string().contains("256"));
    }

    #[test]
    fn generations_ignore_huge_and_malformed_files_and_include_manifest_references() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        for name in [
            "source-18446744073709551615.png",
            "exposure-4294967296-0.png",
            "exposure-999-bad.png",
            "source-invalid.png",
        ] {
            fs::write(temp.path().join(name), b"stray").unwrap();
        }
        let path = temp.path().join("manifest.toml");
        let mut manifest: Manifest = toml::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        manifest.layers[0].exposure = Some("exposure-12-0.png".into());
        fs::write(path, toml::to_string(&manifest).unwrap()).unwrap();
        write(temp.path(), fixture()).unwrap();
        assert_eq!(
            exposure_files(temp.path()),
            [
                "exposure-13-0.png",
                "exposure-13-2.png",
                "exposure-999-bad.png"
            ]
        );
        assert_eq!(
            generation_number("source-4294967295.png"),
            Some(u64::from(u32::MAX))
        );
    }

    #[test]
    fn round_trip_generations_cleanup_and_source_preservation() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("photo.rthid");
        assert_eq!(default_path(&temp.path().join("photo.png")), dir);
        write(&dir, fixture()).unwrap();
        assert!(is_session(&dir));
        let loaded = read(&dir).unwrap();
        assert_equal(&fixture(), &loaded);
        for index in 0..loaded.document.layers().len() {
            assert!(!loaded.document.can_undo(index));
            assert!(!loaded.document.can_redo(index));
        }
        assert_eq!(
            exposure_files(&dir),
            ["exposure-1-0.png", "exposure-1-2.png"]
        );
        let source = dir.join("source-1.png");
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(100);
        fs::File::open(&source).unwrap().set_modified(old).unwrap();
        let modified = fs::metadata(&source).unwrap().modified().unwrap();
        fs::write(dir.join("exposure-8-9.png"), b"orphan").unwrap();
        write(&dir, fixture()).unwrap();
        assert_equal(&fixture(), &read(&dir).unwrap());
        assert_eq!(
            exposure_files(&dir),
            ["exposure-9-0.png", "exposure-9-2.png"]
        );
        assert_eq!(fs::metadata(&source).unwrap().modified().unwrap(), modified);
        let mut empty = fixture();
        empty.document.select(None);
        empty.view.viewport.zoom = Zoom::In(4);
        write(&dir, empty).unwrap();
        let loaded = read(&dir).unwrap();
        assert_eq!(loaded.document.selected(), None);
        assert_eq!(loaded.view.viewport.zoom, Zoom::In(4));
    }

    #[test]
    fn failed_save_before_manifest_preserves_previous_session() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        let before = fs::read(temp.path().join("manifest.toml")).unwrap();
        let mut changed = fixture();
        changed.document.add_layer("Uncommitted".into());
        changed
            .document
            .target_mut()
            .unwrap()
            .exposure
            .add(0, 0, 999);
        let result = write_before_manifest(temp.path(), changed, || {
            Err(io::Error::other("injected before manifest"))
        });
        assert!(result.is_err());
        assert!(temp.path().join("exposure-2-3.png").exists());
        assert_eq!(fs::read(temp.path().join("manifest.toml")).unwrap(), before);
        assert_equal(&fixture(), &read(temp.path()).unwrap());
        write(temp.path(), fixture()).unwrap();
        assert_eq!(
            exposure_files(temp.path()),
            ["exposure-3-0.png", "exposure-3-2.png"]
        );
    }

    #[test]
    fn failed_first_save_leaves_a_directory_the_next_save_reuses() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("photo.rthid");
        let result = write_before_manifest(&dir, fixture(), || Err(io::Error::other("injected")));
        assert!(result.is_err());
        assert!(dir.join("source-1.png").exists());
        // A crash during a pixel write leaves an atomic temporary behind.
        fs::write(dir.join(".rthid-crash.tmp"), b"partial").unwrap();
        assert!(!is_session(&dir));
        assert!(is_saveable(&dir));
        write(&dir, fixture()).unwrap();
        assert_equal(&fixture(), &read(&dir).unwrap());
        assert!(!dir.join("source-1.png").exists());
        fs::create_dir(dir.join("source-9.png")).unwrap();
        fs::remove_file(dir.join("manifest.toml")).unwrap();
        assert!(!is_saveable(&dir));
    }

    #[test]
    fn refuses_broken_newer_and_missing_files_with_file_names() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        for text in ["version = 2", "[broken", "version = 1"] {
            fs::write(dir.join("manifest.toml"), text).unwrap();
            assert!(
                read(dir)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("manifest.toml")
            );
        }
        fs::remove_file(dir.join("manifest.toml")).unwrap();
        write(dir, fixture()).unwrap();
        fs::remove_file(dir.join("source-1.png")).unwrap();
        assert!(
            read(dir)
                .err()
                .unwrap()
                .to_string()
                .contains("source-1.png")
        );
    }

    #[test]
    fn refuses_layer_size_depth_and_missing_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        write(dir, fixture()).unwrap();
        let layer = dir.join("exposure-1-0.png");
        for size in [(1, 1), (68, 65)] {
            let image: ImageBuffer<Luma<u16>, Vec<u16>> = ImageBuffer::new(size.0, size.1);
            image.save(&layer).unwrap();
            assert!(
                read(dir)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("exposure-1-0.png")
            );
        }
        GrayImage::new(67, 65).save(&layer).unwrap();
        assert!(
            read(dir)
                .err()
                .unwrap()
                .to_string()
                .contains("16 bit grayscale")
        );
        fs::remove_file(&layer).unwrap();
        assert!(
            read(dir)
                .err()
                .unwrap()
                .to_string()
                .contains("exposure-1-0.png")
        );
    }

    #[test]
    #[cfg(unix)]
    fn reading_session_rejects_symlinked_pixel_files() {
        for name in ["source-1.png", "exposure-1-0.png"] {
            let temp = tempfile::tempdir().unwrap();
            let dir = temp.path().join("photo.rthid");
            write(&dir, fixture()).unwrap();
            let path = dir.join(name);
            let outside = temp.path().join("outside.png");
            fs::copy(&path, &outside).unwrap();
            fs::remove_file(&path).unwrap();
            std::os::unix::fs::symlink(&outside, &path).unwrap();
            let error = read(&dir).err().unwrap().to_string();
            assert!(error.contains("session files must not be symlinks"));
            assert!(error.contains(name));
        }
    }

    #[test]
    #[cfg(unix)]
    fn session_file_symlinks_cannot_overwrite_the_original_source() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original.png");
        fs::write(&original, b"original image bytes").unwrap();
        let dir = temp.path().join("photo.rthid");
        fs::create_dir(&dir).unwrap();
        {
            let name = "manifest.toml";
            let link = dir.join(name);
            symlink(&original, &link).unwrap();
            assert!(
                write(&dir, fixture())
                    .unwrap_err()
                    .to_string()
                    .contains(name)
            );
            assert_eq!(fs::read(&original).unwrap(), b"original image bytes");
            fs::remove_file(link).unwrap();
        }
    }

    #[test]
    fn manifest_validation_clamps_like_configuration_and_rejects_bad_structure() {
        let temp = tempfile::tempdir().unwrap();
        write(temp.path(), fixture()).unwrap();
        let path = temp.path().join("manifest.toml");
        let text = fs::read_to_string(&path).unwrap();
        let mut value: toml::Value = toml::from_str(&text).unwrap();
        value["settings"]["seed"] = toml::Value::Integer(i64::MAX);
        value["settings"]["strength"] = toml::Value::Float(12.0);
        value["settings"]["zoom"] = toml::Value::Integer(99999);
        fs::write(&path, toml::to_string(&value).unwrap()).unwrap();
        let loaded = read(temp.path()).unwrap();
        assert_eq!(loaded.settings.seed, u32::MAX);
        assert_eq!(loaded.settings.strength, 1.0);
        assert_eq!(loaded.settings.zoom, 400);
        for (field, bad) in [
            ("selected", toml::Value::Integer(999)),
            ("source", toml::Value::String("../source.png".into())),
        ] {
            let mut value: toml::Value = toml::from_str(&text).unwrap();
            value[field] = bad;
            fs::write(&path, toml::to_string(&value).unwrap()).unwrap();
            assert!(
                read(temp.path())
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("manifest.toml")
            );
        }
    }
}
