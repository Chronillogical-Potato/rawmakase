//! X-Trans demosaicing by directional candidates.
//!
//! X-Trans has no regular two-pixel greens, so one isotropic estimate turns fine
//! periodic detail (louvres, grilles) into colour moiré. Instead every direction
//! (horizontal, vertical and both diagonals) gets its own full RGB candidate:
//!
//! 1. Green at red/blue sites by interpolating along the direction between the
//!    nearest greens, corrected by the same-colour curvature along that line.
//! 2. Red and blue from the difference to that direction's green, as on Bayer.
//! 3. Per pixel, the candidates are blended by how smooth their colour difference
//!    (R − G, B − G) is around it. A correct direction leaves neutral detail neutral;
//!    a wrong one leaves a colour pattern, which scores badly.
//!
//! The image is processed in row bands with a mirrored margin so every window fits.
use super::Cfa;
use rayon::prelude::*;

/// Rows of output per band.
const BAND: usize = 128;
/// Margin around a band: green line search 4, colour difference line search 4,
/// then the variation measure and its 3×3 window, one each.
const MARGIN: usize = 10;
/// How far along a line to look for a green or same-colour sample.
const REACH: isize = 4;
/// Horizontal, vertical and the two diagonals as (dx, dy).
const DIRECTIONS: [(isize, isize); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];
/// Floor of the variation measure, in white-normalised units, so that noise on a
/// flat area does not pick a direction.
const VARIATION_FLOOR: f32 = 1e-3;

pub(super) fn demosaic(cfa: &Cfa) -> Vec<[f32; 3]> {
    let w = cfa.width;
    let mut out = vec![[0f32; 3]; w * cfa.height];
    out.par_chunks_mut(w * BAND)
        .enumerate()
        .for_each(|(band, rows)| band_candidates(cfa, band * BAND, rows));
    out
}

/// A band of sensor samples with its mirrored margin.
struct Band {
    width: usize,
    height: usize,
    raw: Vec<f32>,
    colour: Vec<u8>,
}

impl Band {
    fn new(cfa: &Cfa, y0: usize, rows: usize) -> Self {
        let width = cfa.width + 2 * MARGIN;
        let height = rows + 2 * MARGIN;
        let mut raw = Vec::with_capacity(width * height);
        let mut colour = Vec::with_capacity(width * height);
        for py in 0..height {
            let gy = Cfa::reflect(y0 as isize + py as isize - MARGIN as isize, cfa.height);
            for px in 0..width {
                let gx = Cfa::reflect(px as isize - MARGIN as isize, cfa.width);
                raw.push(cfa.data[gy * cfa.width + gx]);
                colour.push(cfa.color(gx, gy));
            }
        }
        Self {
            width,
            height,
            raw,
            colour,
        }
    }

    /// Nearest sample of `colour` along `step` from `i`, as (distance, value).
    #[inline]
    fn along(&self, i: usize, step: isize, colour: u8) -> Option<(f32, f32)> {
        (1..=REACH).find_map(|k| {
            let j = (i as isize + k * step) as usize;
            (self.colour[j] == colour).then(|| (k as f32, self.raw[j]))
        })
    }

    /// Green at `i` along one direction.
    fn green(&self, i: usize, step: isize) -> f32 {
        let own = self.colour[i];
        if own == 1 {
            return self.raw[i];
        }
        let c = self.raw[i];
        let (fwd, back) = (self.along(i, step, 1), self.along(i, -step, 1));
        let (mut est, lo, hi) = match (fwd, back) {
            (Some((a, ga)), Some((b, gb))) => ((ga * b + gb * a) / (a + b), ga.min(gb), ga.max(gb)),
            (Some((_, g)), None) | (None, Some((_, g))) => (g, g, g),
            (None, None) => return c,
        };
        // Same-colour curvature along the line, as the Hamilton–Adams correction.
        if let (Some((a, ca)), Some((b, cb))) =
            (self.along(i, step, own), self.along(i, -step, own))
        {
            est += 0.5 * (c - (ca * b + cb * a) / (a + b));
        }
        est.clamp(lo, hi).max(0.)
    }
}

/// Demosaics one band into `out` (whole rows starting at image row `y0`).
fn band_candidates(cfa: &Cfa, y0: usize, out: &mut [[f32; 3]]) {
    let w = cfa.width;
    let rows = out.len() / w;
    let band = Band::new(cfa, y0, rows);
    let (pw, ph) = (band.width, band.height);
    let candidates = DIRECTIONS.map(|(dx, dy)| {
        let step = dy * pw as isize + dx;
        let rgb = candidate(&band, step);
        let variation = variation(&rgb, pw, ph);
        (rgb, variation)
    });
    for (r, row) in out.chunks_mut(w).enumerate() {
        for (x, px) in row.iter_mut().enumerate() {
            let i = (r + MARGIN) * pw + x + MARGIN;
            let windows = candidates.each_ref().map(|(_, variation)| {
                let window: f32 = [-1isize, 0, 1]
                    .iter()
                    .flat_map(|dy| [-1isize, 0, 1].map(|dx| dy * pw as isize + dx))
                    .map(|o| variation[(i as isize + o) as usize])
                    .sum();
                window + VARIATION_FLOOR
            });
            // Relative to the smoothest candidate, so the best always weighs 1.
            let best = windows.iter().fold(f32::INFINITY, |a, b| a.min(*b));
            let mut sum = [0f32; 3];
            let mut total = 0.;
            for ((rgb, _), window) in candidates.iter().zip(windows) {
                let ratio = best / window;
                let weight = ratio.powi(8);
                for c in 0..3 {
                    sum[c] += rgb[i][c] * weight;
                }
                total += weight;
            }
            *px = sum.map(|v| v / total);
        }
    }
}

/// Full RGB along one direction, valid at distance `MARGIN - 2` from the band edge.
fn candidate(band: &Band, step: isize) -> Vec<[f32; 3]> {
    let (pw, ph) = (band.width, band.height);
    let mut green = vec![0f32; pw * ph];
    for y in REACH as usize..ph - REACH as usize {
        for x in REACH as usize..pw - REACH as usize {
            let i = y * pw + x;
            green[i] = band.green(i, step);
        }
    }
    let mut rgb = vec![[0f32; 3]; pw * ph];
    for y in MARGIN - 2..ph - (MARGIN - 2) {
        for x in MARGIN - 2..pw - (MARGIN - 2) {
            let i = y * pw + x;
            let own = band.colour[i];
            rgb[i][1] = green[i];
            for c in [0u8, 2] {
                if own == c {
                    rgb[i][c as usize] = band.raw[i];
                    continue;
                }
                // Along the direction, between the nearest samples of the colour on
                // both sides, interpolating their difference to green.
                let diff = |j: usize| band.raw[j] - green[j];
                let line = |dir: isize| {
                    (1..=REACH).find_map(|k| {
                        let j = (i as isize + k * dir) as usize;
                        (band.colour[j] == c).then(|| (k as f32, diff(j)))
                    })
                };
                if let (Some((a, da)), Some((b, db))) = (line(step), line(-step)) {
                    rgb[i][c as usize] = (green[i] + (da * b + db * a) / (a + b)).max(0.);
                    continue;
                }
                // Otherwise the nearest samples within radius 1, else 2, weighted by
                // inverse distance.
                let mut sum = 0.;
                let mut total = 0.;
                for radius in 1..=2isize {
                    for dy in -radius..=radius {
                        for dx in -radius..=radius {
                            let j = (i as isize + dy * pw as isize + dx) as usize;
                            if dx.abs().max(dy.abs()) != radius || band.colour[j] != c {
                                continue;
                            }
                            let weight = 1. / ((dx * dx + dy * dy) as f32).sqrt();
                            sum += (band.raw[j] - green[j]) * weight;
                            total += weight;
                        }
                    }
                    if total > 0. {
                        break;
                    }
                }
                rgb[i][c as usize] = (green[i] + if total > 0. { sum / total } else { 0. }).max(0.);
            }
        }
    }
    rgb
}

/// How much R − G and B − G change across each pixel, valid at distance
/// `MARGIN - 1` from the band edge.
fn variation(rgb: &[[f32; 3]], pw: usize, ph: usize) -> Vec<f32> {
    let chroma = |i: usize| [rgb[i][0] - rgb[i][1], rgb[i][2] - rgb[i][1]];
    let mut out = vec![0f32; pw * ph];
    for y in MARGIN - 1..ph - (MARGIN - 1) {
        for x in MARGIN - 1..pw - (MARGIN - 1) {
            let i = y * pw + x;
            let (l, r, u, d) = (chroma(i - 1), chroma(i + 1), chroma(i - pw), chroma(i + pw));
            out[i] = (0..2)
                .map(|c| (l[c] - r[c]).abs() + (u[c] - d[c]).abs())
                .sum();
        }
    }
    out
}
