# Transform panel

Engine 4 renders Lightroom's Transform panel: the manual sliders (`Recipe::transform`, imported from `crs:Perspective*`) and Upright (`Recipe::upright`, imported from `crs:PerspectiveUpright` and `crs:Upright*`). Areas with no source pixel render white, as in Lightroom.

Both are homographies applied after lens correction and before the crop, in the frame the camera recorded: before the photo is rotated or flipped for display, and after the camera's default crop. Camera Raw 18.6 works in that frame, so on a photo the camera turned to portrait, `PerspectiveVertical` keystones across the screen. Lightroom's panel shows the sliders along the displayed photo instead: its Vertical −70 on a photo turned 90° left is stored as `PerspectiveHorizontal` +70. RAWmakase stores them as Lightroom does and shows them the same way (`Transform::displayed`). Upright applies first, then the sliders. Lightroom's manual lens Distortion applies before both, in the same frame ([lens corrections](lens-corrections.md#manual-distortion)).

## Crop and Straighten

The Crop tool (R) works on the photo as shown, after its rotation and flips. The crop is stored in 0–1 coordinates of that frame, straightened: Straighten turns the photo clockwise for a positive angle and enlarges it to leave no white, so any crop inside 0–1 has the photo behind it everywhere.

- **Rotate and Flip** (`develop::turn`, `develop::mirror`) keep the crop on the same part of the photo: Rotate turns the crop with the photo, and Flip mirrors the crop and turns the straighten angle the other way. Rotate turns the photo as shown clockwise or counter-clockwise even under a single flip, where a turn of the recorded photo shows the other way round. Changing lens corrections leaves the crop's numbers as they are. Rotation and flips are kept in the catalog only; as before, XMP carries the crop and angle (`CropLeft` to `CropBottom`, `CropAngle`) but not the orientation.
- **Straighten ruler**: the panel's Ruler, or a Cmd-drag on the photo as in Lightroom, draws a line; on release the angle is set so the line is level, or plumb when it is nearer upright than level, as one History step ("Straighten"). Lines shorter than 10 points are ignored. The angle stays within ±45°.
- **Auto** (next to the Ruler) measures the photo with Upright's analysis and sets the angle Level would roll it by, as one History step ("Straighten", "Auto"); Upright's mode is left alone. It analyses the photo as shown without its Transform (orientation and lens corrections only), off the UI thread, and measures again if the photo is turned, flipped or its lens corrections change meanwhile. On a synthetic tilted horizon and rolled verticals it levels the edges within 1 px over 200 px. A photo with no long near-horizontal edges or verticals gets no angle.
- **X** swaps the crop between portrait and landscape at the same aspect, about its centre, shrinking it to fit the photo when needed. Aspect presets are long side over short in the photo's own orientation; after X the preset is kept as its reciprocal, so dragging a handle keeps the swapped orientation.
- **Overlays**: Grid, Thirds, Diagonal, Triangle, Golden Ratio and Golden Spiral. O cycles them in that order, Shift+O turns the Triangle (2 ways) and Golden Spiral (4 corners). The panel's Overlay menu shows when: Always, Auto (the default: with the pointer over the photo, as Lightroom's Auto Show, while the crop or ruler is dragged, and for 1.5 s after an overlay is picked, so choosing one in the panel shows it) or Never. The ruler shows a grid while it is drawn. The overlay and when it shows are a view preference saved in the session, not part of the edit. Lightroom's Aspect Ratios overlay is not implemented.

X, O and Shift+O work only while the Crop tool is open and no text field has focus; with the Crop tool open X does not reject the photo.

## Constrain Crop

Lightroom's Constrain Crop (`crs:CropConstrainToWarp` 1; the Transform panel's checkbox, `Recipe::constrain_crop`) keeps the white areas that Upright, the Transform sliders and manual Distortion uncover out of the crop. It is not `CropConstrainToUnitSquare`, which only limits Lightroom's crop tool.

Camera Raw 18.7 does not apply the flag when it renders: on the synthetic chart with Vertical +30, Rotate 5, Scale 80 or manual Distortion +50, with no crop, a full crop or a user crop, renders with and without `CropConstrainToWarp="1"` are identical, white areas included. Lightroom constrains the crop in its crop tool and stores the result in `CropLeft` to `CropBottom`. How its tool picks that crop can't be scripted or read from a sidecar, so it was not measured.

RAWmakase applies the constraint while rendering, so it follows every later change to the geometry: the crop as rendered (`Geometry::crop`) is the stored crop when every position in it has a source pixel, and otherwise the largest crop at the stored crop's aspect that has one everywhere and lies inside the stored crop. It shrinks about the stored crop's centre unless moving it keeps more than 0.1% more of its size; for a keystone from Vertical it slides towards the wider edge. A crop lying wholly in the white becomes the largest crop at its aspect anywhere in the photo. Resetting the Transform panel turns Constrain Crop off. Straighten alone never needs it: the photo is already enlarged to leave no white. The stored crop stays as the user drew it, and the Crop tool shows the whole photo around it, white areas included. Lens profiles and built-in lens data never uncover white (their correction is scaled to fill the frame), so the area is set by Upright, the Transform sliders and manual Distortion.

An exported photo's settings carry the crop as rendered with `CropConstrainToWarp="1"`, as Lightroom stores it, so Camera Raw renders the same crop, and reading them back changes nothing.

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
