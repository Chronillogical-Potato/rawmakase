# Masking

Status: experimental and early. It works, but the local sliders are not measured
against Lightroom yet, and details may change.

The Masking tool (Shift+W) works like Lightroom Classic's Masking panel for the
masks that need no AI model: Brush, Linear Gradient, Radial Gradient, Color Range and
Luminance Range, plus **Select Subject** and **Select Background**, which run a model on
this computer (below). Sky, Objects, People and Depth masks are not implemented yet.

## Select Subject and Select Background

Two actions above the Create row make a mask from the photo's salient foreground
(Background is the same selection inverted as a component, so adding a brush to it
still adds coverage). They are also in each mask's Add, Subtract and Intersect menus.
A result is one named mask and one History step with neutral sliders and its overlay
shown; **Regenerate** on a selected generated component replaces only its raster.
Clean up with ordinary Add or Subtract brush components. Subject means the model's
foreground, possibly several objects, not one person; when it finds nothing the
drawer says "No subject found" and nothing is added.

- **On this computer.** The first use asks to download the model (IS-Net general use,
  179 MB, checked against a pinned SHA-256) or to import the file yourself; nothing
  downloads at startup, on opening a catalog or the drawer, or at an update. It runs on
  the CPU through ONNX Runtime, which ships with the app (beside the executable;
  source builds put `libonnxruntime` in the data folder's `runtime/` or name it in
  `RAWMAKASE_ORT_LIB`). No photo leaves the computer. **Remove selection model**
  deletes it; saved masks keep working without it or the runtime.
- **Catalog only.** The result is a raster stored in the catalog, so it needs a catalog
  photo. A catalog of the first format asks to **Upgrade catalog…**: a backup copy is
  written beside it, then it becomes format version 2, which older releases refuse.
  Close it on other computers first.
- **What the model sees:** the photo at the camera's default crop with the camera
  rendering (no tone, colour, profile or geometry edits; spots and red eye included),
  about 1280 pixels on the long side. Sliders and crop edits during or after selecting
  do not matter; changing spots or red eye meanwhile discards the result, as does any
  change to the masks' structure (adding, removing, Undo).
- **Not copied:** masks made from a selection belong to their photo. Copy, Paste and
  Sync leave them out (and say so), and presets cannot hold them.
- **Limits:** one 4096-pixel, 8-bit raster per selection; rasters are never
  garbage-collected, so a catalog only grows with them. Grow/shrink and feather are
  not implemented. No calibrated "faint result" threshold: only an exactly empty
  result counts as no subject.

## Using it

- **Create:** the drawer's Create row, or **K** (Brush), **M** (Linear Gradient),
  **Shift+M** (Radial Gradient), **Shift+J** (Color Range). Gradients are drawn by
  dragging on the photo; Color Range samples the colour you click; Luminance Range
  starts at the brightest half and has Range and Falloff sliders (or click a tone).
- **Mask list:** click to select, double-click to rename, the eye hides a mask's
  effect. The selected mask shows its components with **Add**, **Subtract** and
  **Intersect** menus, **Invert**, **Duplicate** and **Delete**.
- **Brush:** drag to paint; hold **Option/Alt** (or pick Erase) to erase. Size,
  Feather, Flow and Density per brush, brushes **A** and **B** (**/** switches),
  **Auto Mask** keeps the brush to colours like the one under its centre, **[ ]**
  or the mouse wheel over the photo size (with **Option/Alt**, the Erase brush),
  **Shift+[ ]** or Shift-scroll feather.
- **Gradients:** drag a linear gradient's ends to size and turn it, its middle to
  move it; drag a radial gradient's centre to move it and its edge handles to size
  and turn it. Its Feather slider sets the soft edge.
- **O** shows the selected mask as a red overlay.
- Sliders follow Lightroom's order: Amount; Temp, Tint; Exposure, Contrast,
  Highlights, Shadows, Whites, Blacks; Texture, Clarity, Dehaze; Hue, Saturation;
  Sharpness, Noise; Color (hue and saturation). History names them like
  "Mask 2: Exposure".
- Clicks that no mask shape needs still zoom and pan; hold **Space** to pan while
  painting.

## How it renders

- Masks are stored as parameters beside the recipe, like spots (see
  [retouching](retouching.md)), in image space, so they follow crop, straighten, Transform and lens
  corrections. Components combine in order: Add takes the larger weight, Subtract
  removes (not below zero), Intersect takes the smaller; each component and the
  whole mask can be inverted.
- For every rendered pixel the weight of each mask is evaluated at its image
  position: gradients analytically, brushes from an image-space raster (at least four
  pixels per brush radius, up to 2048 on the long side, cached by the strokes), and
  ranges from the pixel's developed colour without local adjustments (Oklab
  chromaticity for Color Range, Oklab lightness for Luminance Range).
- A pixel's adjustment is the sum over its masks of weight × Amount × sliders, run
  through the normal pipeline at the point each global control acts:
  - Exposure scales linear light together with the global Exposure (including the
    DNG exposure ramp's black point), Color tints it.
  - Contrast, Whites, Blacks and Dehaze use the same measured Camera Raw curves as
    the global sliders; Highlights and Shadows use the global local-tone operator at
    the pixel's slider values. A mask covering the whole photo renders like the
    global slider (tested to within 0.003; exact for Exposure, Highlights and
    Shadows).
  - Temp and Tint scale the camera channels by the white balance change of a
    ±50 mired or ±50 tint shift at ±100.
  - Texture and Clarity scale the samples by the local-contrast detail the global
    sliders use.
  - Hue rotates and Saturation scales Oklab chroma after the colour mixer.
  - Sharpness adds to the finishing sharpening per pixel (below zero it softens);
    Noise blends in an edge-aware average (positive values only).
- **GPU:** mask weights (one byte per mask and pixel, up to 16 masks) go to the
  develop shader, which applies every slider except Texture, Clarity, Sharpness and
  Noise; those run around it on the CPU. A hardware test checks the GPU against the
  CPU (largest difference 2e-5). Masks with ranges, Texture, Clarity, Sharpness or
  Noise sample the photo on the CPU instead of keeping it on the device.
- Weights are cached per region by the masks' shapes, so dragging a slider reuses
  them; range masks also depend on the rest of the recipe.

## Not verified against Lightroom

- None of the local sliders have been measured against Camera Raw. Contrast,
  Highlights, Shadows, Whites, Blacks and Dehaze reuse the global measurements; Temp,
  Tint, Hue, Saturation, Color, Texture, Clarity, Sharpness and Noise are our own
  approximations of Lightroom's behaviour.
- Lightroom's gradient and brush feather profiles, Flow build-up and Auto Mask edge
  detection are approximations.
- Color Range's Refine and Luminance Range's smoothness are not calibrated to
  Lightroom's numbers.
- Moiré and Defringe local sliders are not implemented.
