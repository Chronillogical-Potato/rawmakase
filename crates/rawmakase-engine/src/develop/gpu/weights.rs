//! Coefficients shared by GPU invocations, computed once per axis on the CPU.
//! Matches image's Lanczos3 support, normalization and boundary convention.
pub(super) fn lanczos_axis(input: u32, output: u32) -> (u32, Vec<f32>) {
    let ratio = input as f32 / output as f32;
    let scale = ratio.max(1.);
    let support = 3. * scale;
    let stride = (2. * support).ceil() as u32 + 3;
    let mut data = vec![0.; (stride * output) as usize];
    for x in 0..output {
        let center = (x as f32 + 0.5) * ratio;
        let start = ((center - support).floor() as i64).clamp(0, input as i64 - 1) as u32;
        let end = ((center + support).ceil() as i64).clamp(start as i64 + 1, input as i64) as u32;
        let row = &mut data[(x * stride) as usize..((x + 1) * stride) as usize];
        row[0] = start as f32;
        row[1] = (end - start) as f32;
        let weights = &mut row[2..2 + (end - start) as usize];
        let mut sum = 0.;
        for (i, weight) in weights.iter_mut().enumerate() {
            let v = ((start as usize + i) as f32 - (center - 0.5)) / scale;
            let sinc = |v: f32| {
                let v = v * std::f32::consts::PI;
                if v == 0. { 1. } else { v.sin() / v }
            };
            *weight = if v.abs() < 3. {
                sinc(v) * sinc(v / 3.)
            } else {
                0.
            };
            sum += *weight;
        }
        for weight in weights {
            *weight /= sum;
        }
    }
    (stride, data)
}
