//! The retouched camera image: the highlight-recovered image with every operation
//! applied in order. Previews keep it between renders and, when operations change,
//! recompute only the 256-pixel tiles those changes reach; exports build it at once.
use super::{
    RetouchOp,
    heal::{self, PixelRect, Placed},
};
use crate::{develop::ImageFrame, raw::CameraImage};
use anyhow::{Result, ensure};
use std::{
    collections::BTreeSet,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

const TILE: i32 = 256;

/// `base` with `ops` applied, built from scratch (exports).
pub(crate) fn apply(base: &CameraImage, ops: &[RetouchOp]) -> CameraImage {
    let frame = ImageFrame::new(base);
    let mut out = base.clone();
    out.recovered = Default::default();
    for op in ops {
        heal::apply(&mut out, &Placed::new(op, &frame));
    }
    out
}
/// Tiles, as (column, row), covering a pixel rectangle.
fn tiles(rect: &PixelRect) -> impl Iterator<Item = (i32, i32)> + use<> {
    let (x0, y0) = (rect[0].max(0) / TILE, rect[1].max(0) / TILE);
    let (x1, y1) = ((rect[2] - 1).max(0) / TILE, (rect[3] - 1).max(0) / TILE);
    (y0..=y1).flat_map(move |y| (x0..=x1).map(move |x| (x, y)))
}
fn touches(dirty: &BTreeSet<(i32, i32)>, rect: &PixelRect) -> bool {
    rect[2] > rect[0] && rect[3] > rect[1] && tiles(rect).any(|t| dirty.contains(&t))
}
/// Tiles that must be recomputed when `before` becomes `after`: those of changed
/// operations, then, until nothing changes, those written by any operation that reads
/// or writes a dirty tile. Recomputing exactly these tiles, and rerunning the
/// operations that touch them in order, gives the same image as starting over.
pub(crate) fn dirty_tiles(
    frame: &ImageFrame,
    before: &[RetouchOp],
    after: &[RetouchOp],
) -> BTreeSet<(i32, i32)> {
    let mut dirty = BTreeSet::new();
    for i in 0..before.len().max(after.len()) {
        let (b, a) = (before.get(i), after.get(i));
        if b != a {
            for op in [b, a].into_iter().flatten() {
                let rect = Placed::new(op, frame).dest();
                dirty.extend(tiles(&rect));
            }
        }
    }
    let placed: Vec<Placed> = after.iter().map(|op| Placed::new(op, frame)).collect();
    loop {
        let size = dirty.len();
        for p in &placed {
            if p.reads().iter().any(|r| touches(&dirty, r)) {
                dirty.extend(tiles(&p.dest()));
            }
        }
        if dirty.len() == size {
            return dirty;
        }
    }
}
/// Pixel rectangles of `tiles`, clipped to a `width` × `height` image.
fn tile_rects(tiles: &BTreeSet<(i32, i32)>, width: u32, height: u32) -> Vec<PixelRect> {
    tiles
        .iter()
        .map(|(x, y)| {
            [
                x * TILE,
                y * TILE,
                ((x + 1) * TILE).min(width as i32),
                ((y + 1) * TILE).min(height as i32),
            ]
        })
        .filter(|r| r[2] > r[0] && r[3] > r[1])
        .collect()
}
/// The preview's retouched image, updated incrementally.
#[derive(Default)]
pub(crate) struct RetouchCache {
    base: Option<Arc<CameraImage>>,
    ops: Vec<RetouchOp>,
    image: Option<Arc<CameraImage>>,
    /// The previous image (weakly, so its pixels are freed) and the rectangles where
    /// the current one differs from it.
    change: Option<(Weak<CameraImage>, Vec<PixelRect>)>,
}
impl RetouchCache {
    /// `base` with `ops` applied. The previous result is reused where no change
    /// reaches it.
    pub(crate) fn get(
        &mut self,
        base: &Arc<CameraImage>,
        ops: &[RetouchOp],
        cancel: &AtomicBool,
    ) -> Result<Arc<CameraImage>> {
        let same_base = self.base.as_ref().is_some_and(|b| Arc::ptr_eq(b, base));
        let previous = self.image.clone().unwrap_or_else(|| base.clone());
        let previous_ops = if same_base {
            self.ops.clone()
        } else {
            Vec::new()
        };
        if same_base && previous_ops == ops {
            return Ok(previous);
        }
        let frame = ImageFrame::new(base);
        let dirty = dirty_tiles(&frame, &previous_ops, ops);
        let rects = tile_rects(&dirty, base.width, base.height);
        let image = if ops.is_empty() {
            base.clone()
        } else {
            let start = if same_base { &previous } else { base };
            let mut out = CameraImage::clone(start);
            out.recovered = Default::default();
            // Back to the recovered pixels in dirty tiles, then rerun what reaches them.
            let width = base.width as usize;
            for r in &rects {
                for y in r[1]..r[3] {
                    let row = y as usize * width;
                    let (a, b) = (row + r[0] as usize, row + r[2] as usize);
                    out.pixels[a..b].copy_from_slice(&base.pixels[a..b]);
                }
            }
            for op in ops {
                ensure!(!cancel.load(Ordering::Relaxed), "Render superseded");
                let placed = Placed::new(op, &frame);
                if placed.reads().iter().any(|r| touches(&dirty, r)) {
                    heal::apply(&mut out, &placed);
                }
            }
            Arc::new(out)
        };
        // Another photo's image shares nothing with this one.
        self.change = same_base.then(|| (Arc::downgrade(&previous), rects));
        self.base = Some(base.clone());
        self.ops = ops.to_vec();
        self.image = Some(image.clone());
        Ok(image)
    }
    /// What the last change replaced, and where the images differ.
    /// Whether `image` is what the last change replaced; then the current image
    /// differs from it only in the returned rectangles.
    pub(crate) fn changed_from(&self, image: &Arc<CameraImage>) -> Option<&[PixelRect]> {
        let (previous, rects) = self.change.as_ref()?;
        std::ptr::eq(previous.as_ptr(), Arc::as_ptr(image)).then_some(rects.as_slice())
    }
}
