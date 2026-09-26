# Transform panel

Engine 4 renders Lightroom's manual Transform sliders (`Recipe::transform`, imported from `crs:Perspective*`). Upright modes are not implemented, and XMP settings with a non-zero `PerspectiveUpright` are still rejected.

The transform is a homography applied in the oriented frame after lens correction and before the crop. Areas with no source pixel render white, as in Lightroom. Coordinates are centred, y down, in units of the long edge. The forward (source-to-output) matrices were fitted to Camera Raw 18.6 renders of DSCF7853 by minimizing pixel error over all eight homography terms. The fits were exact apart from these structures:

| Slider (Lightroom value s/100) | Forward matrix |
|---|---|
| Vertical v | `[[1, 0, 0], [0, k(v), 0], [0, −v, 1]]`, k(s) = 1 + 0.347 s² + 0.334 s⁴ (fitted at 0.5 and 1) |
| Horizontal h | `[[k(h), 0, 0], [0, 1, 0], [−h, 0, 1]]` |
| Rotate θ | rotation by θ degrees |
| Aspect a | x × 2^(−0.137 a), y × 2^(0.137 a) |
| Scale | uniform scale |
| X / Y Offset | translation by 0.811 × image width / height; positive Y moves up |

The sliders compose as offset · scale · aspect · rotate · horizontal · vertical. The composition order was not measured separately.

Scorecard against the Camera Raw renders (MAE; the untransformed render of the same photo is 0.0167): Vertical ±50/±100 0.017–0.019, Horizontal ±50/±100 0.018–0.020, Rotate ±5 0.019–0.020, Aspect ±50 0.018–0.021, Scale 80/120 0.015–0.018, Offsets ±50 0.011–0.013.
