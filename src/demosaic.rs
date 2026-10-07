//! RAWmakase's own demosaicing of unpacked sensor data (LibRaw only unpacks).
//!
//! Input is the visible-area CFA, black-subtracted, normalized to white and white
//! balanced, with a 48×48 colour pattern (0 red, 1 green, 2 blue), which covers Bayer
//! (2 or 16 pixel) and X-Trans (6 pixel) periods. Both Bayer and
//! X-Trans use the same two passes:
//!
//! 1. Green at red/blue sites, interpolated along the direction of least gradient
//!    (Hamilton–Adams with Laplacian correction on Bayer, where the colour two pixels
//!    away is the same; gradient-weighted neighbours on X-Trans).
//! 2. Red and blue everywhere from the colour difference to green (R − G, B − G) of
//!    the nearest same-colour samples, which keeps edges aligned across channels.
use rayon::prelude::*;

/// Side of the colour pattern: a common multiple of the 2, 6 and 16 pixel periods.
pub(crate) const PATTERN: usize = 48;

pub(crate) struct Cfa<'a> {
    pub data: &'a [f32],
    pub width: usize,
    pub height: usize,
    pub pattern: &'a [u8; PATTERN * PATTERN],
}
impl Cfa<'_> {
    #[inline]
    fn color(&self, x: usize, y: usize) -> u8 {
        self.pattern[(y % PATTERN) * PATTERN + x % PATTERN]
    }
    /// Mirrors coordinates at the borders, which keeps the Bayer colour parity.
    #[inline]
    fn reflect(v: isize, n: usize) -> usize {
        let n = n as isize;
        let v = if v < 0 { -v } else { v };
        let v = if v >= n { 2 * (n - 1) - v } else { v };
        v.clamp(0, n - 1) as usize
    }
    #[inline]
    fn at(&self, x: isize, y: isize) -> f32 {
        let (x, y) = (Self::reflect(x, self.width), Self::reflect(y, self.height));
        self.data[y * self.width + x]
    }
    #[inline]
    fn color_at(&self, x: isize, y: isize) -> u8 {
        let (x, y) = (Self::reflect(x, self.width), Self::reflect(y, self.height));
        self.color(x, y)
    }
    /// A Bayer pattern repeats every two pixels with two greens per 2×2 block.
    fn is_bayer(&self) -> bool {
        (0..PATTERN).all(|y| (0..PATTERN).all(|x| self.color(x, y) == self.color(x % 2, y % 2)))
            && (0..2)
                .flat_map(|y| (0..2).map(move |x| (x, y)))
                .filter(|(x, y)| self.color(*x, *y) == 1)
                .count()
                == 2
    }
}

pub(crate) fn demosaic(cfa: &Cfa) -> Vec<[f32; 3]> {
    let green = if cfa.is_bayer() {
        green_bayer(cfa)
    } else {
        green_general(cfa)
    };
    colour_difference(cfa, &green)
}

fn green_bayer(cfa: &Cfa) -> Vec<f32> {
    let (w, h) = (cfa.width, cfa.height);
    let mut g = vec![0f32; w * h];
    g.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let y = y as isize;
        for (x, out) in row.iter_mut().enumerate() {
            let xi = x as isize;
            let c = cfa.at(xi, y);
            if cfa.color(x, y as usize) == 1 {
                *out = c;
                continue;
            }
            let (l, r) = (cfa.at(xi - 1, y), cfa.at(xi + 1, y));
            let (u, d) = (cfa.at(xi, y - 1), cfa.at(xi, y + 1));
            let lap_h = 2. * c - cfa.at(xi - 2, y) - cfa.at(xi + 2, y);
            let lap_v = 2. * c - cfa.at(xi, y - 2) - cfa.at(xi, y + 2);
            let gh = (l + r) * 0.5 + lap_h * 0.25;
            let gv = (u + d) * 0.5 + lap_v * 0.25;
            let dh = (l - r).abs() + lap_h.abs();
            let dv = (u - d).abs() + lap_v.abs();
            // Blend by inverse gradient instead of a hard switch, avoiding zipper edges.
            let (wh, wv) = (1. / (dh + 1e-5), 1. / (dv + 1e-5));
            let est = (gh * wh + gv * wv) / (wh + wv);
            // Keep the estimate within the neighbouring greens (no overshoot halos).
            let lo = l.min(r).min(u).min(d);
            let hi = l.max(r).max(u).max(d);
            *out = est.clamp(lo, hi).max(0.);
        }
    });
    g
}

fn green_general(cfa: &Cfa) -> Vec<f32> {
    let (w, h) = (cfa.width, cfa.height);
    let mut g = vec![0f32; w * h];
    g.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let yi = y as isize;
        for (x, out) in row.iter_mut().enumerate() {
            let xi = x as isize;
            if cfa.color(x, y) == 1 {
                *out = cfa.at(xi, yi);
                continue;
            }
            // Greens among the 8 neighbours, weighted by how well each agrees with the
            // green on the opposite side (a directional gradient test).
            let mut sum = 0.;
            let mut total = 0.;
            for (dx, dy) in [(1, 0), (0, 1), (1, 1), (1, -1)] {
                let a = (xi + dx, yi + dy);
                let b = (xi - dx, yi - dy);
                let ga = cfa.color_at(a.0, a.1) == 1;
                let gb = cfa.color_at(b.0, b.1) == 1;
                let diag = if dx != 0 && dy != 0 { 0.7 } else { 1. };
                match (ga, gb) {
                    (true, true) => {
                        let (va, vb) = (cfa.at(a.0, a.1), cfa.at(b.0, b.1));
                        let wgt = diag / ((va - vb).abs() + 1e-4);
                        sum += (va + vb) * 0.5 * wgt;
                        total += wgt;
                    }
                    (true, false) | (false, true) => {
                        let v = if ga {
                            cfa.at(a.0, a.1)
                        } else {
                            cfa.at(b.0, b.1)
                        };
                        let wgt = diag * 0.5 / (local_contrast(cfa, xi, yi) + 1e-4);
                        sum += v * wgt;
                        total += wgt;
                    }
                    _ => {}
                }
            }
            *out = if total > 0. {
                sum / total
            } else {
                cfa.at(xi, yi)
            };
        }
    });
    g
}
fn local_contrast(cfa: &Cfa, x: isize, y: isize) -> f32 {
    let mut lo = f32::INFINITY;
    let mut hi = 0f32;
    for dy in -1..=1 {
        for dx in -1..=1 {
            if cfa.color_at(x + dx, y + dy) == 1 {
                let v = cfa.at(x + dx, y + dy);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    if hi >= lo { hi - lo } else { 0. }
}

fn colour_difference(cfa: &Cfa, green: &[f32]) -> Vec<[f32; 3]> {
    let (w, h) = (cfa.width, cfa.height);
    let g = |x: isize, y: isize| green[Cfa::reflect(y, h) * w + Cfa::reflect(x, w)];
    let mut out = vec![[0f32; 3]; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let yi = y as isize;
        for (x, px) in row.iter_mut().enumerate() {
            let xi = x as isize;
            let gc = g(xi, yi);
            let own = cfa.color(x, y);
            px[1] = gc;
            for c in [0u8, 2] {
                if own == c {
                    px[c as usize] = cfa.at(xi, yi);
                    continue;
                }
                // Nearest same-colour samples within radius 1, else 2, weighted by
                // inverse distance, interpolating their difference to green.
                let mut sum = 0.;
                let mut total = 0.;
                for radius in 1..=2isize {
                    for dy in -radius..=radius {
                        for dx in -radius..=radius {
                            if dx.abs().max(dy.abs()) != radius
                                || cfa.color_at(xi + dx, yi + dy) != c
                            {
                                continue;
                            }
                            let wgt = 1. / ((dx * dx + dy * dy) as f32).sqrt();
                            sum += (cfa.at(xi + dx, yi + dy) - g(xi + dx, yi + dy)) * wgt;
                            total += wgt;
                        }
                    }
                    if total > 0. {
                        break;
                    }
                }
                px[c as usize] = (gc + if total > 0. { sum / total } else { 0. }).max(0.);
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bayer_pattern() -> [u8; PATTERN * PATTERN] {
        std::array::from_fn(|i| match ((i / PATTERN) % 2, i % 2) {
            (0, 0) => 0,
            (1, 1) => 2,
            _ => 1,
        })
    }
    fn xtrans_pattern() -> [u8; PATTERN * PATTERN] {
        const X: [[u8; 6]; 6] = [
            [1, 1, 0, 1, 1, 2],
            [1, 1, 2, 1, 1, 0],
            [2, 0, 1, 0, 2, 1],
            [1, 1, 2, 1, 1, 0],
            [1, 1, 0, 1, 1, 2],
            [0, 2, 1, 2, 0, 1],
        ];
        std::array::from_fn(|i| X[(i / PATTERN) % 6][(i % PATTERN) % 6])
    }
    fn mosaic(
        pattern: &[u8; PATTERN * PATTERN],
        w: usize,
        h: usize,
        f: impl Fn(usize, usize) -> [f32; 3],
    ) -> Vec<f32> {
        (0..w * h)
            .map(|i| {
                f(i % w, i / w)[pattern[((i / w) % PATTERN) * PATTERN + (i % w) % PATTERN] as usize]
            })
            .collect()
    }
    #[test]
    fn flat_colour_is_reconstructed_exactly() {
        for pattern in [bayer_pattern(), xtrans_pattern()] {
            let data = mosaic(&pattern, 24, 18, |_, _| [0.4, 0.3, 0.2]);
            let cfa = Cfa {
                data: &data,
                width: 24,
                height: 18,
                pattern: &pattern,
            };
            for p in demosaic(&cfa) {
                assert!(
                    (p[0] - 0.4).abs() < 1e-5
                        && (p[1] - 0.3).abs() < 1e-5
                        && (p[2] - 0.2).abs() < 1e-5,
                    "{p:?}"
                );
            }
        }
    }
    #[test]
    fn gray_edges_stay_gray_and_sharp() {
        for pattern in [bayer_pattern(), xtrans_pattern()] {
            let data = mosaic(
                &pattern,
                32,
                24,
                |x, _| if x < 16 { [0.1; 3] } else { [0.8; 3] },
            );
            let cfa = Cfa {
                data: &data,
                width: 32,
                height: 24,
                pattern: &pattern,
            };
            let out = demosaic(&cfa);
            for y in 4..20 {
                for x in 2..30 {
                    let p = out[y * 32 + x];
                    // Away from the edge the colour is exact; at the edge no strong colour fringe.
                    let spread = p.iter().fold(0f32, |a, b| a.max(*b))
                        - p.iter().fold(1f32, |a, b| a.min(*b));
                    assert!(spread < 0.25, "{x},{y}: {p:?}");
                    if !(13..=18).contains(&x) {
                        assert!(spread < 1e-4, "{x},{y}: {p:?}");
                    }
                }
            }
        }
    }
    #[test]
    fn recognises_bayer_and_xtrans() {
        let b = bayer_pattern();
        let x = xtrans_pattern();
        assert!(
            Cfa {
                data: &[],
                width: 0,
                height: 0,
                pattern: &b
            }
            .is_bayer()
        );
        assert!(
            !Cfa {
                data: &[],
                width: 0,
                height: 0,
                pattern: &x
            }
            .is_bayer()
        );
    }
}
