# Color mixer and color grading

## Measurement

Camera Raw 18.6 rendered every color mixer slider at ±100, and Saturation and Vibrance at ±50, on nine photos (Fujifilm X100F, Sony A7 II and A7CR, chosen to cover skin, foliage, sky and magenta). Color grading was rendered for shadows, midtones, highlights and global at six hues (Saturation 50) plus Saturation 100 and Luminance ±50, on four photos. `scripts/camera-raw-sweep.py` produces the renders and `scripts/lightroom-scorecard.py` scores them.

## Color mixer, Saturation and Vibrance (engine 4)

Their effect on the default rendering is a hue/saturation/value lookup in linear ProPhoto RGB. The change depends on hue, saturation and brightness. `src/develop/color_mixer.rs` applies measured tables (`color_mixer.bin`: 36 hues × 6 saturations × 6 values, hue shift and log2 saturation/value factors per slider extreme), interpolated trilinearly and scaled linearly by slider position. Several sliders add their changes. With each photo left out of the fit, every slider reproduces Camera Raw to 0.0002–0.0066 MAE, 3–5× closer than leaving the image unchanged. On three of the photos, the extra error over the default render drops from +0.0022 to +0.0003 on average, and for the worst slider (Orange Luminance −100) from +0.0153 to +0.0016. Blue and purple are the least covered bands.

## Color grading (engine 4)

Each region's tint is a per-channel gain of linear ProPhoto RGB that depends only on the pixel's luminance. Luminance sliders shift sRGB-encoded ProPhoto values by luminance. `src/develop/color_grade.rs` interpolates the six measured hues and scales saturation linearly. With each photo left out, the tint reproduces Camera Raw to 0.0036 (highlights) – 0.0088 (shadows) MAE, and Luminance ±50 to 0.0036–0.0063. Only Lightroom's default Blending (50) and Balance (0) were measured, so recipes with other values keep the previous operator. On the sweeps, those settings still show the previous operator's +0.015 to +0.034 extra error.

Legacy split-toning records (SplitToning keys without ColorGrade keys) imply Blending 100 in both Camera Raw and RAWmakase, so they also use the previous operator.
