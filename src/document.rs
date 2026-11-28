//! The source image is immutable and is never written back to disk.
//! Edits live in layers composed onto it before the grid and dither stages.
//!
//! Dodge and burn use one i16 exposure offset per source pixel.
//! Applying an operation and its inverse cannot undo clamping at 0 or 255.
//! Instead, visible layer offsets are summed and applied once, so strokes
//! accumulate without clamping loss. Each map will later serialize as one
//! 16 bit grayscale PNG.
//!
//! Offsets use units of 1/256 stop. A total offset s multiplies each RGB
//! channel by 2^(s / 256), then rounds and clamps it to 0..=255.
//! Alpha stays unchanged.
//!
//! Maps use 64x64 tiles held in Arc. A missing tile means all zero, so an
//! untouched layer allocates no pixel storage. Writes use Arc::make_mut,
//! copying only tiles shared with a history state.
//!
//! Undo belongs to each layer because independent exposure maps should keep
//! Ctrl+Z on the layer being painted. Structure changes are not undoable and
//! deletion asks first. This replaces linear document snapshots, which rewound
//! other layers and removed newly created layers unexpectedly.
//!
//! Edits use source pixel coordinates. The grid is rebuilt from the source
//! on every zoom, pan, rotation, flip or card change.
//! Every pixel change bumps the document revision, which keys the composed image
//! cache. Caches therefore never need to be cleared by hand for edits.
//!
//! Sessions use a directory with manifest.toml,
//! a copy of the shrunk source, and one exposure PNG per nonempty layer.
//! Ctrl+S saves the session and never overwrites the source image.

use std::sync::Arc;

use image::RgbaImage;

/// Edge length of an exposure tile in source pixels.
pub const TILE: u32 = 64;
/// Offset that doubles a channel value.
pub const STOP: i16 = 256;
/// PNG value representing a zero exposure offset.
pub(crate) const LUMA16_MIDPOINT: u16 = 32768;
/// Maximum number of undo entries kept per layer.
pub const HISTORY_LIMIT: usize = 100;

/// Sparse exposure offsets shared with stroke snapshots.
#[derive(Clone, Debug, PartialEq)]
pub struct ExposureMap {
    width: u32,
    height: u32,
    tiles: Vec<Option<Arc<[i16; (TILE * TILE) as usize]>>>,
}

impl ExposureMap {
    /// Creates a map with no allocated pixel storage.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            tiles: vec![None; width.div_ceil(TILE) as usize * height.div_ceil(TILE) as usize],
        }
    }

    /// Source width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Source height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    fn position(&self, x: u32, y: u32) -> (usize, usize) {
        (
            (y / TILE) as usize * self.width.div_ceil(TILE) as usize + (x / TILE) as usize,
            ((y % TILE) * TILE + x % TILE) as usize,
        )
    }

    /// Reads an offset, treating absent tiles as zero. Panics outside the map.
    pub fn get(&self, x: u32, y: u32) -> i16 {
        assert!(x < self.width && y < self.height);
        let (tile, pixel) = self.position(x, y);
        self.tiles[tile].as_ref().map_or(0, |tile| tile[pixel])
    }

    /// Adds with saturation, ignoring coordinates outside the source.
    pub fn add(&mut self, x: u32, y: u32, delta: i16) {
        if x >= self.width || y >= self.height {
            return;
        }
        let (tile, pixel) = self.position(x, y);
        let tile = self.tiles[tile].get_or_insert_with(|| Arc::new([0; (TILE * TILE) as usize]));
        let tile = Arc::make_mut(tile);
        tile[pixel] = tile[pixel].saturating_add(delta);
    }

    /// Encodes signed offsets around the PNG midpoint.
    pub fn to_luma16(&self) -> Vec<u16> {
        let mut pixels = Vec::with_capacity(self.width as usize * self.height as usize);
        for y in 0..self.height {
            for x in 0..self.width {
                pixels.push((i32::from(self.get(x, y)) + i32::from(LUMA16_MIDPOINT)) as u16);
            }
        }
        pixels
    }

    /// Restores offsets while leaving tiles containing only zero offsets absent.
    ///
    /// # Panics
    /// Panics if the pixel count does not equal width times height.
    pub fn from_luma16(width: u32, height: u32, pixels: &[u16]) -> Self {
        assert_eq!(pixels.len(), width as usize * height as usize);
        let mut map = Self::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let offset = (i32::from(pixels[(y * width + x) as usize])
                    - i32::from(LUMA16_MIDPOINT)) as i16;
                if offset != 0 {
                    map.add(x, y, offset);
                }
            }
        }
        map
    }

    /// Whether every tile is absent.
    pub fn is_empty(&self) -> bool {
        self.tiles.iter().all(Option::is_none)
    }
}

/// A named exposure map with independent visibility.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    /// Label shown in the layer list.
    pub name: String,
    /// Whether this layer contributes to composition.
    pub visible: bool,
    /// Offsets in source pixel coordinates.
    pub exposure: ExposureMap,
    undo: Vec<ExposureMap>,
    redo: Vec<ExposureMap>,
}

impl Layer {
    /// Creates a visible layer with no exposure changes.
    pub fn new(name: String, width: u32, height: u32) -> Self {
        Self {
            name,
            visible: true,
            exposure: ExposureMap::new(width, height),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

/// Layers ordered from bottom to top and their current selection.
#[derive(Clone, Debug, PartialEq)]
pub struct Document {
    width: u32,
    height: u32,
    layers: Vec<Layer>,
    selected: Option<usize>,
    revision: u64,
}

impl Document {
    /// Creates an unedited document matching the source dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            layers: Vec::new(),
            selected: None,
            revision: 0,
        }
    }

    /// Restores layers with empty stroke stacks after validating dimensions and selection.
    pub fn from_layers(
        width: u32,
        height: u32,
        mut layers: Vec<Layer>,
        selected: Option<usize>,
    ) -> Result<Self, &'static str> {
        if selected.is_some_and(|index| index >= layers.len()) {
            return Err("invalid layer selection");
        }
        if layers
            .iter()
            .any(|layer| (layer.exposure.width(), layer.exposure.height()) != (width, height))
        {
            return Err("layer dimensions differ from source");
        }
        for layer in &mut layers {
            layer.undo.clear();
            layer.redo.clear();
        }
        Ok(Self {
            width,
            height,
            layers,
            selected,
            revision: 0,
        })
    }

    /// Layers in composition order, bottom first.
    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Explicit selection, if any.
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Uses the first available numbered layer name, filling gaps after deletion.
    pub fn next_layer_name(&self) -> String {
        let mut k = 1;
        while self
            .layers
            .iter()
            .any(|layer| layer.name == format!("Layer {k}"))
        {
            k += 1;
        }
        format!("Layer {k}")
    }

    /// Adds a layer at the top and selects it.
    pub fn add_layer(&mut self, name: String) -> usize {
        self.layers.push(Layer::new(name, self.width, self.height));
        self.revision += 1;
        let index = self.layers.len() - 1;
        self.selected = Some(index);
        index
    }

    /// Removes a valid layer and adjusts or clears its selection.
    pub fn remove_layer(&mut self, index: usize) {
        if index >= self.layers.len() {
            return;
        }
        self.layers.remove(index);
        self.revision += 1;
        self.selected = self.selected.and_then(|selected| {
            if selected == index {
                None
            } else {
                Some(selected - usize::from(selected > index))
            }
        });
    }

    /// Changes visibility when the layer exists.
    pub fn set_visible(&mut self, index: usize, visible: bool) {
        if let Some(layer) = self.layers.get_mut(index)
            && layer.visible != visible
        {
            layer.visible = visible;
            self.revision += 1;
        }
    }

    /// Reorders valid layers while preserving the selected layer.
    pub fn move_layer(&mut self, from: usize, to: usize) {
        if from >= self.layers.len() || to >= self.layers.len() || from == to {
            return;
        }
        self.revision += 1;
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        self.selected = self.selected.map(|selected| {
            if selected == from {
                to
            } else if from < selected && selected <= to {
                selected - 1
            } else if to <= selected && selected < from {
                selected + 1
            } else {
                selected
            }
        });
    }

    /// Sets or clears selection, ignoring invalid indices.
    pub fn select(&mut self, index: Option<usize>) {
        if index.is_none_or(|index| index < self.layers.len()) {
            self.selected = index;
        }
    }

    /// Edits use the selection, falling back to the top layer.
    pub fn target(&self) -> Option<usize> {
        self.selected.or_else(|| self.layers.len().checked_sub(1))
    }

    /// Mutable access for building test fixtures before painting.
    #[cfg(test)]
    pub fn target_mut(&mut self) -> Option<&mut Layer> {
        let index = self.target()?;
        self.layers.get_mut(index)
    }

    /// Cache key for pixel and structure changes, excluding selection.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Paints in place after the caller clones the map and before `commit_stroke`.
    pub fn paint(&mut self, index: usize, paint: impl FnOnce(&mut ExposureMap)) {
        if let Some(layer) = self.layers.get_mut(index) {
            paint(&mut layer.exposure);
            self.revision += 1;
        }
    }

    /// Ends painting, saving the map captured before the first dab only if it changed.
    pub fn commit_stroke(&mut self, index: usize, before: ExposureMap) -> bool {
        let Some(layer) = self.layers.get_mut(index) else {
            return false;
        };
        if layer.exposure == before {
            return false;
        }
        if layer.undo.len() == HISTORY_LIMIT {
            layer.undo.remove(0);
        }
        layer.undo.push(before);
        layer.redo.clear();
        true
    }

    /// Restores the preceding stroke map without changing layer structure.
    pub fn undo(&mut self, index: usize) -> bool {
        let Some(layer) = self.layers.get_mut(index) else {
            return false;
        };
        let Some(map) = layer.undo.pop() else {
            return false;
        };
        layer.redo.push(std::mem::replace(&mut layer.exposure, map));
        self.revision += 1;
        true
    }

    /// Restores the next stroke map without changing layer structure.
    pub fn redo(&mut self, index: usize) -> bool {
        let Some(layer) = self.layers.get_mut(index) else {
            return false;
        };
        let Some(map) = layer.redo.pop() else {
            return false;
        };
        layer.undo.push(std::mem::replace(&mut layer.exposure, map));
        self.revision += 1;
        true
    }

    /// Whether the layer has a preceding stroke map.
    pub fn can_undo(&self, index: usize) -> bool {
        self.layers
            .get(index)
            .is_some_and(|layer| !layer.undo.is_empty())
    }

    /// Whether the layer has a next stroke map.
    pub fn can_redo(&self, index: usize) -> bool {
        self.layers
            .get(index)
            .is_some_and(|layer| !layer.redo.is_empty())
    }

    /// Applies summed visible offsets, avoiding a copy when no map contributes.
    pub fn compose(&self, source: &RgbaImage) -> Option<RgbaImage> {
        debug_assert_eq!(source.dimensions(), (self.width, self.height));
        debug_assert!(
            self.layers.iter().all(|layer| {
                (layer.exposure.width(), layer.exposure.height()) == (self.width, self.height)
            }),
            "layer exposure dimensions must match the document"
        );
        let layers: Vec<_> = self
            .layers
            .iter()
            .filter(|layer| layer.visible && !layer.exposure.is_empty())
            .collect();
        let first = layers.first()?;
        let mut output = source.clone();
        let columns = self.width.div_ceil(TILE) as usize;
        for index in 0..first.exposure.tiles.len() {
            if !layers
                .iter()
                .any(|layer| layer.exposure.tiles[index].is_some())
            {
                continue;
            }
            let left = (index % columns) as u32 * TILE;
            let top = (index / columns) as u32 * TILE;
            for y in top..(top + TILE).min(self.height) {
                for x in left..(left + TILE).min(self.width) {
                    let offset: i32 = layers
                        .iter()
                        .map(|layer| i32::from(layer.exposure.get(x, y)))
                        .sum();
                    if offset == 0 {
                        continue;
                    }
                    let factor = (offset as f32 / f32::from(STOP)).exp2();
                    let pixel = output.get_pixel_mut(x, y);
                    for channel in &mut pixel.0[..3] {
                        *channel = (f32::from(*channel) * factor).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        Some(output)
    }
}

#[cfg(test)]
/// Compares persisted document content independently of caches and stroke stacks.
pub(crate) fn assert_same_content(actual: &Document, expected: &Document) {
    assert_eq!(
        (actual.width, actual.height, actual.selected),
        (expected.width, expected.height, expected.selected)
    );
    assert_eq!(actual.layers.len(), expected.layers.len());
    for (actual, expected) in actual.layers.iter().zip(&expected.layers) {
        assert_eq!(
            (&actual.name, actual.visible, &actual.exposure),
            (&expected.name, expected.visible, &expected.exposure)
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn stroke(doc: &mut Document, index: usize, delta: i16) {
        let before = doc.layers()[index].exposure.clone();
        doc.paint(index, |map| map.add(0, 0, delta));
        assert!(doc.commit_stroke(index, before));
    }

    #[test]
    fn stroke_stacks_are_independent_and_structure_is_not_undoable() {
        let mut doc = Document::new(1, 1);
        doc.add_layer("Layer 1".into());
        stroke(&mut doc, 0, STOP);
        doc.add_layer("Layer 2".into());
        stroke(&mut doc, 1, STOP * 2);
        assert!(doc.undo(1));
        assert!(!doc.undo(1));
        assert_eq!(doc.layers()[0].exposure.get(0, 0), STOP);
        assert!(doc.undo(0));
        assert!(doc.can_redo(0));
        assert!(doc.can_redo(1));
        stroke(&mut doc, 1, STOP);
        assert!(doc.can_redo(0));
        assert!(!doc.can_redo(1));
        doc.move_layer(1, 0);
        doc.set_visible(0, false);
        let revision = doc.revision();
        doc.select(None);
        doc.select(Some(0));
        assert_eq!(doc.revision(), revision);
        assert!(doc.undo(0));
        assert_eq!(doc.layers().len(), 2);
        assert_eq!(doc.layers()[0].name, "Layer 2");
        assert!(!doc.layers()[0].visible);
        assert_eq!(doc.selected(), Some(0));
        assert!(doc.can_redo(1));
    }

    #[test]
    fn saturated_stroke_keeps_redo_and_loading_clears_stacks() {
        let mut doc = Document::new(1, 1);
        doc.add_layer("Layer 1".into());
        stroke(&mut doc, 0, i16::MAX);
        stroke(&mut doc, 0, -1);
        assert!(doc.undo(0));
        let before = doc.layers()[0].exposure.clone();
        doc.paint(0, |map| map.add(0, 0, 1));
        assert!(!doc.commit_stroke(0, before));
        assert!(doc.can_redo(0));
        assert_eq!(doc.layers[0].undo.len(), 1);
        let loaded = Document::from_layers(1, 1, doc.layers.clone(), Some(0)).unwrap();
        assert_same_content(&loaded, &doc);
        assert!(!loaded.can_undo(0));
        assert!(!loaded.can_redo(0));
    }

    #[test]
    fn each_layer_keeps_the_newest_hundred_strokes() {
        let mut doc = Document::new(1, 1);
        doc.add_layer("Layer 1".into());
        doc.add_layer("Layer 2".into());
        for _ in 0..HISTORY_LIMIT + 5 {
            for index in 0..2 {
                stroke(&mut doc, index, 1);
            }
        }
        for index in 0..2 {
            assert_eq!(doc.layers[index].undo.len(), HISTORY_LIMIT);
            for _ in 0..HISTORY_LIMIT {
                assert!(doc.undo(index));
            }
            assert!(!doc.undo(index));
            assert_eq!(doc.layers()[index].exposure.get(0, 0), 5);
            for _ in 0..HISTORY_LIMIT {
                assert!(doc.redo(index));
            }
            assert!(!doc.redo(index));
            assert_eq!(doc.layers()[index].exposure.get(0, 0), 105);
        }
    }

    #[test]
    fn stroke_snapshots_share_unwritten_tiles() {
        let mut doc = Document::new(128, 64);
        doc.add_layer("Layer 1".into());
        doc.paint(0, |map| {
            map.add(0, 0, STOP);
            map.add(64, 0, STOP);
        });
        let before = doc.layers()[0].exposure.clone();
        doc.paint(0, |map| map.add(0, 0, STOP));
        doc.paint(0, |map| map.add(1, 0, STOP));
        assert!(doc.commit_stroke(0, before));
        let before = &doc.layers[0].undo[0];
        let after = &doc.layers[0].exposure;
        assert_eq!(before.get(0, 0), STOP);
        assert_eq!(before.get(1, 0), 0);
        assert_eq!(after.get(0, 0), STOP * 2);
        assert!(!Arc::ptr_eq(
            before.tiles[0].as_ref().unwrap(),
            after.tiles[0].as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.tiles[1].as_ref().unwrap(),
            after.tiles[1].as_ref().unwrap()
        ));
    }

    #[test]
    fn exposure_png_conversion_preserves_offsets_and_sparse_zero_tiles() {
        let pixels = vec![32768; 130 * 65];
        let empty = ExposureMap::from_luma16(130, 65, &pixels);
        assert!(empty.is_empty());
        assert_eq!(empty.to_luma16(), pixels);
        let mut map = ExposureMap::new(130, 65);
        map.add(129, 64, i16::MIN);
        map.add(0, 0, i16::MAX);
        let restored = ExposureMap::from_luma16(130, 65, &map.to_luma16());
        assert_eq!(restored, map);
        assert!(restored.tiles[1].is_none());
        assert!(Document::from_layers(1, 1, Vec::new(), Some(0)).is_err());
        assert!(Document::from_layers(1, 1, vec![Layer::new("wrong".into(), 2, 2)], None).is_err());
    }

    #[test]
    fn layer_names_fill_the_first_available_number() {
        let mut document = Document::new(1, 1);
        assert_eq!(document.next_layer_name(), "Layer 1");
        document.add_layer(document.next_layer_name());
        document.add_layer(document.next_layer_name());
        document.add_layer(document.next_layer_name());
        assert_eq!(document.next_layer_name(), "Layer 4");
        document.remove_layer(1);
        assert_eq!(document.next_layer_name(), "Layer 2");
    }

    #[test]
    fn sparse_tiles_saturate_and_ignore_outside_coordinates() {
        let mut map = ExposureMap::new(130, 65);
        assert_eq!((map.width(), map.height()), (130, 65));
        assert!(map.is_empty());
        map.add(130, 0, STOP);
        map.add(0, 65, STOP);
        assert!(map.is_empty());
        for (x, y) in [(0, 0), (63, 63), (64, 64), (129, 64)] {
            assert_eq!(map.get(x, y), 0);
            map.add(x, y, i16::MAX);
            map.add(x, y, 1);
            assert_eq!(map.get(x, y), i16::MAX);
            map.add(x, y, i16::MIN);
            map.add(x, y, i16::MIN);
            assert_eq!(map.get(x, y), i16::MIN);
        }
        assert!(!map.is_empty());
        assert_eq!(map.get(65, 64), 0);
    }

    #[test]
    fn layer_operations_preserve_selection_and_target_fallback() {
        let mut doc = Document::new(1, 1);
        assert_eq!(doc.target(), None);
        assert!(doc.target_mut().is_none());
        for name in ["a", "b", "c"] {
            doc.add_layer(name.into());
        }
        assert_eq!(doc.selected(), Some(2));
        doc.move_layer(0, 2);
        assert_eq!(doc.selected(), Some(1));
        doc.move_layer(2, 0);
        assert_eq!(doc.selected(), Some(2));
        doc.move_layer(2, 0);
        assert_eq!(doc.selected(), Some(0));
        assert_eq!(doc.layers()[0].name, "c");
        doc.move_layer(8, 0);
        doc.move_layer(0, 8);
        doc.select(Some(8));
        assert_eq!(doc.selected(), Some(0));
        doc.select(Some(2));
        doc.remove_layer(0);
        assert_eq!(doc.selected(), Some(1));
        doc.remove_layer(8);
        doc.remove_layer(1);
        assert_eq!(doc.selected(), None);
        assert_eq!(doc.target(), Some(0));
        doc.add_layer("d".into());
        doc.select(None);
        assert_eq!(doc.target(), Some(1));
        doc.remove_layer(1);
        doc.remove_layer(0);
        assert_eq!(doc.target(), None);
    }

    #[test]
    fn composition_skips_absent_empty_and_hidden_maps() {
        let source = RgbaImage::from_pixel(1, 1, Rgba([100, 50, 0, 42]));
        let mut doc = Document::new(1, 1);
        assert!(doc.compose(&source).is_none());
        doc.add_layer("empty".into());
        assert!(doc.compose(&source).is_none());
        doc.target_mut().unwrap().exposure.add(0, 0, STOP);
        doc.set_visible(0, false);
        assert!(doc.compose(&source).is_none());
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "layer exposure dimensions must match the document")]
    fn composition_rejects_mismatched_layer_dimensions() {
        let mut doc = Document::new(1, 1);
        doc.add_layer("empty".into());
        let layer = doc.target_mut().unwrap();
        layer.exposure = ExposureMap::new(2, 1);
        layer.visible = false;
        doc.compose(&RgbaImage::new(1, 1));
    }

    #[test]
    fn composition_sums_before_rounding_and_clamping_and_preserves_alpha() {
        let source = RgbaImage::from_pixel(65, 65, Rgba([100, 50, 0, 42]));
        let mut doc = Document::new(65, 65);
        doc.add_layer("first".into());
        let map = &mut doc.target_mut().unwrap().exposure;
        map.add(0, 0, STOP);
        map.add(64, 64, -STOP);
        map.add(63, 63, STOP * 10);
        let composed = doc.compose(&source).unwrap();
        assert_eq!(composed.get_pixel(0, 0).0, [200, 100, 0, 42]);
        assert_eq!(composed.get_pixel(64, 64).0, [50, 25, 0, 42]);
        assert_eq!(composed.get_pixel(63, 63).0, [255, 255, 0, 42]);
        assert_eq!(composed.get_pixel(1, 0), source.get_pixel(1, 0));
        assert_eq!(composed.get_pixel(64, 0), source.get_pixel(64, 0));
        doc.add_layer("second".into());
        doc.target_mut().unwrap().exposure.add(0, 0, STOP);
        assert_eq!(
            doc.compose(&source).unwrap().get_pixel(0, 0).0,
            [255, 200, 0, 42]
        );
        doc.target_mut().unwrap().exposure.add(0, 0, -STOP * 2);
        assert_eq!(
            doc.compose(&source).unwrap().get_pixel(0, 0),
            source.get_pixel(0, 0)
        );
        assert_eq!(source.get_pixel(0, 0).0, [100, 50, 0, 42]);
    }
}
