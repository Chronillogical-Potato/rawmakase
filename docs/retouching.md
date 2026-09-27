# Spot removal

The Remove tool (Q) works like Lightroom Classic's Remove panel in Heal and Clone
modes. The AI Remove mode is not implemented yet.

## Using it

- **Click** a spot: RAWmakase adds a circle and picks a source nearby automatically.
- **Drag** to paint a brushed area; its source is picked when you release.
- **Cmd-drag** (Ctrl-drag on Linux) places a spot and drags its source by hand.
- Drag a spot's pin or circle to move it; drag its source circle to move the source.
- **/** picks the next best source for the selected spot.
- **[ ]** change the size (of the selected spot, or of new ones); **Shift+[ ]** the
  feather. **Delete** removes the selected spot, **H** hides the pins.
- **A** turns on Visualize Spots, a black-and-white view of fine detail where dust
  stands out; its Threshold slider shows fainter detail.
- Hold **Space** to pan while the tool is open.
- The drawer's Mode, Size, Feather and Opacity apply to the selected spot, or to new
  spots when none is selected.

Paste Settings and presets leave a photo's spots alone, as Lightroom's defaults do.

## How it renders

- Spots are stored as parameters (`retouch`) beside the recipe, not in it: in the
  catalog's `local_edits` table, or for photos opened directly in
  `photo.ARW.rawmakase-local.json` next to the sidecar. The recipe stays schema 6, so
  earlier releases open the photo with every other edit, just without spots and
  masks. Positions are in image space: the photo as the camera oriented it, within its default crop, before lens
  correction, Transform, crop and straightening. Spots stay on their dust when those
  change. Sizes are fractions of the long edge.
- Operations apply in order to the linear, highlight-recovered camera image before
  anything else, so every later edit, and export at full resolution, sees the
  retouched pixels.
- **Clone** blends the source in with the feathered shape.
- **Heal** copies the source, then adds a membrane: the difference between
  destination and source on a one-pixel ring around the shape, extended inward by
  solving Laplace's equation (multigrid V-cycles, so large brushed areas solve as
  quickly as small spots). This is done on log values: on test data, a texture
  healed from a darker area had an RMS residual of 0.014 in log values, 0.028 with
  ln(1 + x) and 0.035 in linear values. Log values reproduce a smooth gradient
  within 1% (0.01 EV); linear values would be exact there.
- **Automatic source:** candidates on rings at 1.5–6 radii (of the brushed area's
  size for brushes) are scored on a reduced copy of the neighbourhood by how well the
  border matches (SSD of log values on a ring around the shape), how similar the
  texture is (gradient energy), and penalties for overlapping the spot itself, other
  spots and clipped highlights; the best is refined by a local search. On a synthetic
  chart (dust on a gradient and on a lit texture, through a real camera profile) the
  healed spots differ from the clean render by a mean ΔE00 of 0.42; the dust was
  10.2 (`tests/color/retouch.rs`).
- **Previews** keep the retouched image and recompute only the 256-pixel tiles a
  change reaches (and anything that reads them, until nothing more changes), then
  patch those areas of the preview pyramid. Exports build the retouched image at
  once. Tests check that the tiles match a full rebuild and that Fit, 100% regions
  and exports agree.

## Not verified against Lightroom

- Lightroom's exact feather profile, its automatic source choice and its heal
  algorithm are not public; results look alike but are not measured against Camera
  Raw.
- Previews keep one full-resolution retouched copy of the photo in memory while it
  has spots.
