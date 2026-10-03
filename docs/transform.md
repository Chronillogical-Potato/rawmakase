# Transform panel

Engine 4 renders Lightroom's Transform panel: the manual sliders (`Recipe::transform`, imported from `crs:Perspective*`) and Upright (`Recipe::upright`, imported from `crs:PerspectiveUpright` and `crs:Upright*`). Areas with no source pixel render white, as in Lightroom.

Both are homographies applied after lens correction and before the crop, in the frame the camera recorded: before the photo is rotated or flipped for display, and after the camera's default crop. Camera Raw 18.6 works in that frame, so on a photo the camera turned to portrait, `PerspectiveVertical` keystones across the screen. Lightroom's panel shows the sliders along the displayed photo instead: its Vertical −70 on a photo turned 90° left is stored as `PerspectiveHorizontal` +70. RAWmakase stores them as Lightroom does and shows them the same way (`Transform::displayed`). Upright applies first, then the sliders. Lightroom's manual lens Distortion applies before both, in the same frame ([lens corrections](lens-corrections.md#manual-distortion)).

## Upright

Lightroom stores the correction for every mode, `crs:UprightTransform_0` to `_5`, indexed by the `PerspectiveUpright` code: 0 Off, 1 Auto, 2 Full, 3 Level, 4 Vertical, 5 Guided. Each is a row-major forward (source-to-output) homography in 0–1 coordinates of the recorded frame. Camera Raw renders the stored matrix as it is: replacing it with a translation moves the render by exactly that, and a wrong `UprightDependentDigest` does not make it recompute. RAWmakase keeps all six corrections, so switching modes needs no new analysis, and keeps the other `Upright*` settings to write back.

Checked against Camera Raw renders of five photos (Sony A7 II and Fujifilm X100F; landscape, both portrait orientations; Level, Vertical and Full; with and without lens profiles): the stored matrices reproduce Camera Raw's geometry within 0.0002 of the image size, and RAWmakase's renders within 1.3 px at 2000 px, the same as the untransformed renders.

A preset that names only an Upright mode applies, and the app analyses each photo it is applied to. A photo's own settings (sidecar or catalog) without Lightroom's stored corrections, and Guided without a stored correction, are reported as unsupported.

## Upright analysis

For new edits, `develop::upright` finds straight edges in a 1024-pixel luminance copy of the displayed photo (an LSD-style detector), then the vertical vanishing point (edges within 20° of vertical, weighted by squared length, with a prior against strong tilts) and a horizontal one orthogonal to it. Lightroom's corrections are camera rotations K·R·K⁻¹ at focal length f = 35mm-equivalent / 36 in long-edge units (fitted to its stored corrections within 1e-9): Level rolls, Vertical takes the vertical vanishing point to vertical, Full also pans to the facade turned least, and Auto corrects part of the tilt. Lightroom then enlarges the result to fill the frame when that takes at most 110%; otherwise Level keeps its size and the other modes fit its width.

Against Lightroom's own corrections on 160 of the author's photos (median, at 2000 px): Level 13 px, Vertical 58 px, Auto 59 px, Full 186 px. Most of the Vertical and Auto difference is framing; the straightening itself usually agrees within 10 px.

## Sliders

Coordinates are centred, y down, in units of the long edge of the recorded frame. The forward matrix is offset · scale · aspect · perspective · rotate, measured by fitting homographies to Camera Raw renders of slider pairs (each other order was 3–10 times worse):

| Slider (Lightroom value / 100) | Forward matrix |
|---|---|
| Rotate θ | rotation by θ degrees, applied first |
| Vertical v, Horizontal h | with q = (h, v), s = \|q\|: `[[I + e(s) q qᵀ / s², 0], [−qᵀ, 1]]`, e(s) = 0.0391 s² + 0.9251 s³ − 0.2827 s⁴ (fitted to Vertical at ±10 to 100 and five Vertical + Horizontal pairs; within 0.0009) |
| Aspect a | x × 2^(−0.137 a), y × 2^(0.137 a) |
| Scale | uniform scale |
| X / Y Offset | translation by 0.811 × image width / height; positive Y moves up |

Checked against 33 Camera Raw renders of three photos (single sliders, pairs, and with Upright), RAWmakase's geometry is within 1.1 px at 2000 px on photos without a lens profile and 2.7 px with one, the same as the untransformed renders.

Before this, the sliders applied Vertical before Rotate, used e(s) = 0.347 s² + 0.334 s⁴ for each axis separately, and acted on the rotated photo, which put portrait photos off by up to 470 px.
