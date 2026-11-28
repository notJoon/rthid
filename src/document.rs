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
//! Undo uses a capped linear history of document snapshots. Cloning a
//! document copies tile handles rather than pixels. Undo and redo move a
//! cursor, and a new edit drops redo states. Command replay was rejected
//! because undo cost grows with history and old sessions would change
//! when the brush algorithm changes.
//!
//! Edits use source pixel coordinates. The grid is rebuilt from the source
//! on every zoom, pan, rotation, flip or card change.
//! Every pixel change bumps the history revision, which keys the composed image
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
/// Maximum number of history states kept, including the current one.
pub const HISTORY_LIMIT: usize = 100;

/// Sparse exposure offsets shared between document snapshots.
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
}

impl Layer {
    /// Creates a visible layer with no exposure changes.
    pub fn new(name: String, width: u32, height: u32) -> Self {
        Self {
            name,
            visible: true,
            exposure: ExposureMap::new(width, height),
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
}

impl Document {
    /// Creates an unedited document matching the source dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            layers: Vec::new(),
            selected: None,
        }
    }

    /// Restores layers after validating their dimensions and explicit selection.
    pub fn from_layers(
        width: u32,
        height: u32,
        layers: Vec<Layer>,
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
        Ok(Self {
            width,
            height,
            layers,
            selected,
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
        if let Some(layer) = self.layers.get_mut(index) {
            layer.visible = visible;
        }
    }

    /// Reorders valid layers while preserving the selected layer.
    pub fn move_layer(&mut self, from: usize, to: usize) {
        if from >= self.layers.len() || to >= self.layers.len() {
            return;
        }
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
    /// Callers holding a [`History`] should use [`History::select`] or select inside `edit`.
    pub fn select(&mut self, index: Option<usize>) {
        if index.is_none_or(|index| index < self.layers.len()) {
            self.selected = index;
        }
    }

    /// Edits use the selection, falling back to the top layer.
    pub fn target(&self) -> Option<usize> {
        self.selected.or_else(|| self.layers.len().checked_sub(1))
    }

    /// Mutable access to the layer an edit should affect.
    pub fn target_mut(&mut self) -> Option<&mut Layer> {
        let index = self.target()?;
        self.layers.get_mut(index)
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

/// Capped document snapshots with a monotonic cache revision.
pub struct History {
    states: Vec<Document>,
    cursor: usize,
    revision: u64,
    redo_tail: Vec<Document>,
    dropped_oldest: Option<Document>,
}

impl History {
    /// Starts history with a single document.
    pub fn new(document: Document) -> Self {
        Self {
            states: vec![document],
            cursor: 0,
            revision: 0,
            redo_tail: Vec::new(),
            dropped_oldest: None,
        }
    }

    /// Snapshot at the undo cursor.
    pub fn current(&self) -> &Document {
        &self.states[self.cursor]
    }

    /// Changes selection in place without adding history or bumping the pixel revision.
    /// Selection does not change pixels, so composition and dither caches stay valid.
    /// Undo restores the selection stored in older snapshots.
    pub fn select(&mut self, index: Option<usize>) {
        self.states[self.cursor].select(index);
    }

    /// Cache key that increases on every history change except selection.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Adds a snapshot, keeping removed redo and oldest states until the next change.
    /// `discard_if_unchanged` can restore them if this edit is reversed by `amend`.
    pub fn edit(&mut self, f: impl FnOnce(&mut Document)) {
        let mut document = self.current().clone();
        f(&mut document);
        self.redo_tail = self.states.split_off(self.cursor + 1);
        self.dropped_oldest = None;
        self.states.push(document);
        if self.states.len() > HISTORY_LIMIT {
            self.dropped_oldest = Some(self.states.remove(0));
        }
        self.cursor = self.states.len() - 1;
        self.revision += 1;
    }

    /// Updates the current stroke without adding a snapshot.
    /// Valid only during a gesture begun with `edit` at the current cursor,
    /// with no undo, redo or reset in between.
    pub fn amend(&mut self, f: impl FnOnce(&mut Document)) {
        debug_assert!(
            !self.can_redo() && self.cursor > 0,
            "amend requires a gesture begun with edit at the current cursor"
        );
        f(&mut self.states[self.cursor]);
        self.revision += 1;
    }

    /// Drops an unchanged snapshot and restores states removed by its `edit`.
    /// Revision increases only when a snapshot is dropped. Amend keeps the removed
    /// states available, while another edit, undo, redo or reset forgets them.
    pub fn discard_if_unchanged(&mut self) -> bool {
        if self.cursor == 0 || self.states[self.cursor] != self.states[self.cursor - 1] {
            return false;
        }
        self.states.remove(self.cursor);
        self.cursor -= 1;
        if let Some(oldest) = self.dropped_oldest.take() {
            self.states.insert(0, oldest);
            self.cursor += 1;
        }
        self.states.append(&mut self.redo_tail);
        self.revision += 1;
        true
    }

    /// Moves to the previous snapshot if one exists.
    pub fn undo(&mut self) -> bool {
        self.redo_tail.clear();
        self.dropped_oldest = None;
        if !self.can_undo() {
            return false;
        }
        self.cursor -= 1;
        self.revision += 1;
        true
    }

    /// Moves to the next snapshot if one exists.
    pub fn redo(&mut self) -> bool {
        self.redo_tail.clear();
        self.dropped_oldest = None;
        if !self.can_redo() {
            return false;
        }
        self.cursor += 1;
        self.revision += 1;
        true
    }

    /// Whether an older snapshot is available.
    pub fn can_undo(&self) -> bool {
        self.cursor > 0
    }

    /// Whether a newer snapshot is available.
    pub fn can_redo(&self) -> bool {
        self.cursor + 1 < self.states.len()
    }

    /// Replaces history for a newly loaded source without resetting the revision.
    pub fn reset(&mut self, document: Document) {
        self.redo_tail.clear();
        self.dropped_oldest = None;
        self.states = vec![document];
        self.cursor = 0;
        self.revision += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

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
    fn unchanged_snapshot_is_discarded_only_when_equal() {
        let mut history = History::new(Document::new(1, 1));
        assert!(!history.discard_if_unchanged());
        assert_eq!(history.revision(), 0);
        history.edit(|doc| {
            doc.add_layer("Layer 1".into());
        });
        assert!(!history.discard_if_unchanged());
        assert_eq!(history.revision(), 1);
        history.edit(|_| {});
        assert!(history.discard_if_unchanged());
        assert_eq!(history.revision(), 3);
        assert!(!history.can_redo());
        assert!(!history.discard_if_unchanged());
        assert_eq!(history.revision(), 3);
        assert!(history.undo());
        assert!(history.current().layers().is_empty());
    }

    #[test]
    fn returning_drag_restores_redo_and_oldest_states() {
        for at_cap in [false, true] {
            let mut doc = Document::new(1, 1);
            doc.add_layer("one".into());
            doc.add_layer("two".into());
            let mut history = History::new(doc);
            let edits = if at_cap { HISTORY_LIMIT - 1 } else { 2 };
            for index in 0..edits {
                history.edit(|doc| doc.layers[0].name = index.to_string());
            }
            // Exercise restoration of the oldest state when there is no redo tail.
            let states = history.states.clone();
            let cursor = history.cursor;
            history.edit(|doc| doc.move_layer(1, 0));
            history.amend(|doc| doc.move_layer(0, 1));
            assert!(history.discard_if_unchanged());
            assert_eq!(history.states, states);
            assert_eq!(history.cursor, cursor);

            assert!(history.undo());
            let states = history.states.clone();
            let cursor = history.cursor;
            history.edit(|doc| doc.move_layer(1, 0));
            history.amend(|doc| doc.move_layer(0, 1));
            assert!(history.discard_if_unchanged());
            assert_eq!(history.states, states);
            assert_eq!(history.cursor, cursor);
            assert!(history.redo());
            assert_eq!(history.current().layers()[0].name, (edits - 1).to_string());
        }
    }

    #[test]
    fn another_history_operation_forgets_removed_states() {
        for operation in ["edit", "undo", "redo", "reset"] {
            let mut history = History::new(Document::new(1, 1));
            history.edit(|doc| {
                doc.add_layer("one".into());
            });
            history.edit(|doc| {
                doc.add_layer("two".into());
            });
            history.undo();
            history.edit(|_| {});
            assert!(!history.redo_tail.is_empty());
            match operation {
                "edit" => history.edit(|_| {}),
                "undo" => {
                    history.undo();
                }
                "redo" => {
                    history.redo();
                }
                "reset" => history.reset(Document::new(1, 1)),
                _ => unreachable!(),
            }
            assert!(history.redo_tail.is_empty());
            assert!(history.dropped_oldest.is_none());
        }
    }

    #[test]
    fn selection_changes_neither_history_nor_revision() {
        let mut history = History::new(Document::new(1, 1));
        history.edit(|doc| {
            doc.add_layer("one".into());
        });
        let revision = history.revision();
        history.select(None);
        assert_eq!(history.current().selected(), None);
        history.select(Some(0));
        assert_eq!(history.current().selected(), Some(0));
        assert_eq!(history.revision(), revision);
        assert_eq!(history.states.len(), 2);
        assert!(history.undo());
        assert!(history.current().layers().is_empty());
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
    fn snapshots_copy_only_written_tiles() {
        let mut document = Document::new(128, 64);
        document.add_layer("Exposure".into());
        let map = &mut document.target_mut().unwrap().exposure;
        map.add(0, 0, STOP);
        map.add(64, 0, STOP);
        let mut history = History::new(document);
        history.edit(|doc| doc.target_mut().unwrap().exposure.add(0, 0, STOP));
        history.amend(|doc| doc.target_mut().unwrap().exposure.add(1, 0, STOP));
        let before = &history.states[0].layers[0].exposure;
        let after = &history.current().layers[0].exposure;
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
    fn history_moves_branches_amends_and_resets_with_increasing_revisions() {
        let mut history = History::new(Document::new(1, 1));
        assert!(!history.undo());
        assert!(!history.redo());
        assert_eq!(history.revision(), 0);
        history.edit(|doc| {
            doc.add_layer("one".into());
        });
        assert_eq!(history.revision(), 1);
        history.amend(|doc| {
            doc.add_layer("two".into());
        });
        assert_eq!(history.states.len(), 2);
        assert_eq!(history.revision(), 2);
        assert!(history.undo());
        assert_eq!(history.revision(), 3);
        assert!(history.current().layers().is_empty());
        assert!(history.redo());
        assert_eq!(history.revision(), 4);
        assert_eq!(history.current().layers().len(), 2);
        assert!(!history.redo());
        assert_eq!(history.revision(), 4);
        assert!(history.undo());
        history.edit(|doc| {
            doc.add_layer("branch".into());
        });
        assert_eq!(history.revision(), 6);
        assert!(!history.can_redo());
        assert!(!history.redo());
        history.reset(Document::new(2, 2));
        assert_eq!(history.revision(), 7);
        assert!(!history.can_undo());
        assert!(!history.can_redo());
        assert_eq!(history.states.len(), 1);
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "amend requires a gesture begun with edit at the current cursor")]
    fn amend_after_undo_is_invalid() {
        let mut history = History::new(Document::new(1, 1));
        history.edit(|_| {});
        history.edit(|_| {});
        assert!(history.undo());
        history.amend(|_| {});
    }

    #[test]
    fn history_cap_keeps_the_newest_states() {
        let mut history = History::new(Document::new(0, 0));
        for index in 0..HISTORY_LIMIT + 5 {
            history.edit(|doc| {
                doc.add_layer(index.to_string());
            });
        }
        assert_eq!(history.states.len(), HISTORY_LIMIT);
        assert_eq!(history.current().layers().len(), HISTORY_LIMIT + 5);
        for _ in 1..HISTORY_LIMIT {
            assert!(history.undo());
        }
        assert!(!history.undo());
        assert_eq!(history.current().layers().len(), 6);
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
