# Color mixer and color grading

## Measurement

Camera Raw 18.6 rendered every color mixer slider at ±100, and Saturation and Vibrance at ±50, on nine photos (Fujifilm X100F, Sony A7 II and A7CR, chosen to cover skin, foliage, sky and magenta). Color grading was rendered for shadows, midtones, highlights and global at six hues (Saturation 50) plus Saturation 100 and Luminance ±50, on four photos. `scripts/camera-raw-sweep.py` produces the renders and `scripts/lightroom-scorecard.py` scores them.

## Color mixer, Saturation and Vibrance (engine 4)

Their effect on the default rendering is a hue/saturation/value lookup in linear ProPhoto RGB. The change depends on hue, saturation and brightness. `src/develop/color_mixer.rs` applies measured tables (`color_mixer.bin`: 36 hues × 6 saturations × 6 values, hue shift and log2 saturation/value factors per slider extreme), interpolated trilinearly. Slider position scales the hue shift, the value factor's log and positive saturation's log linearly. Negative saturation scales the saturation factor itself linearly: Camera Raw's response at −25 and −50 is close to linear in the factor, and scaling the log of a strong measured desaturation removed up to twice as much color (Blue −25 on one photo lowered mean saturation by 0.050 instead of Camera Raw's 0.024). Several sliders add their changes.

Checked with Camera Raw 18.6 on three photos (X100F ×2, A7 II) for single bands at −25/−50/−100/+50, all bands at −25/−50/+50, a pair and one real multi-band edit. Mean extra error over each photo's default render, before → after: all bands −50 +0.0019 → −0.0001, all bands −25 +0.0023 → 0.0000, the multi-band edit +0.0018 → +0.0001, Orange −50 +0.0014 → +0.0001; positive sliders and −100 unchanged. The Lightroom X100F scorecard went from 0.0093 to 0.0090 (HSL reference 0.0178 → 0.0113, portrait mix 0.0152 → 0.0120), with no reference worse. With each photo left out of the fit, every slider reproduces Camera Raw to 0.0002–0.0066 MAE, 3–5× closer than leaving the image unchanged. On three of the photos, the extra error over the default render drops from +0.0022 to +0.0003 on average, and for the worst slider (Orange Luminance −100) from +0.0153 to +0.0016. Blue and purple are the least covered bands.

Both the mixer and grading run after the tone curves (basic curves and point curves). With them before the point curve, Lightroom references that combine grading with a faded point curve scored worse (e.g. global blue 0.0206, against 0.0083 after).

## Color grading (engine 4)

Each region's tint is a per-channel gain of linear ProPhoto RGB that depends only on the pixel's luminance. Luminance sliders shift sRGB-encoded ProPhoto values by luminance. `src/develop/color_grade.rs` interpolates the six measured hues and scales saturation linearly. With each photo left out, the tint reproduces Camera Raw to 0.0036 (highlights) – 0.0088 (shadows) MAE, and Luminance ±50 to 0.0036–0.0063. Only Lightroom's default Blending (50) and Balance (0) were measured, so recipes with other values keep the previous operator. On the sweeps, those settings still show the previous operator's +0.015 to +0.034 extra error.

Legacy split-toning records (SplitToning keys without ColorGrade keys) imply Blending 100 in both Camera Raw and RAWmakase, so they also use the previous operator.
