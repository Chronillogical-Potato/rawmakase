# MIDI control surfaces (Loupedeck+)

RAWmakase listens for a MIDI control surface on macOS and Windows. The built-in
defaults match the **Loupedeck+** (USB 0x2EC2:0x0002), which shows up as a MIDI
port named `Loupedeck+` and sends channel-1 messages. Dials and faders send
relative control changes (`1` is clockwise or up, `127` counter-clockwise or
down); buttons send `note_on` and `note_off`. The code is in
[src/app/control_surface.rs](../src/app/control_surface.rs). Linux builds have no
MIDI backend and ignore all of this.

The listener finds the device again when it is plugged back in. It works while
you are in Develop; dials do nothing in the Library grid. A dial turn that
continues without a 400 ms pause is one History step, named like the slider
("Exposure +0.50"), so one Cmd+Z undoes the turn.

## Default mapping

| Control | CC | Does |
|---|---|---|
| Exposure, Blacks, Whites | 33, 34, 35 | the slider of that name |
| Saturation, Vibrance | 36, 37 | the slider of that name |
| Temperature, Tint | 38, 39 | Temp (in mireds, clockwise is warmer) and Tint |
| Highlights, Shadows | 40, 44 | the slider of that name |
| Clarity, Contrast | 45, 46 | the slider of that name |
| Faders P1–P8 | 17–24 | Color Mixer bands Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta |
| Control Dial | 48 | previous / next photo in Develop and the Loupe |

Dials turn one point (0.01) per tick; Exposure 0.02 EV; Tint one unit.
The faders turn the channel the Color Mixer's Hue / Sat / Lum selector shows
(Hue when it shows "All"), or each band's gray mix in Black & White.
The Control Dial moves at once on the first tick of a turn, then once every
`photo_detent` (default 2) ticks.

| Button | Note | Does |
|---|---|---|
| Shift, Ctrl, Command, Alt | 66, 67, 68, 69 | held modifiers for the next button |
| Up, Down, Left, Right | 76, 77, 78, 79 | arrow keys |
| P1–P5 | 80–84 | 1–5 stars |
| P6 | 85 | clear the rating (0) |
| P7, P8 | 86, 87 | pick (P), reject (X) |
| Export | 88 | Cmd+Shift+E |
| Copy, Paste | 92, 93 | Cmd+Shift+C / V (copy / paste settings) |
| Undo, Redo | 95, 96 | Cmd+Z, Cmd+Shift+Z |
| Screen Mode | 97 | J (clipping overlay) |
| Hue, Sat, Lum | 98, 99, 100 | show that channel in the Color Mixer |
| Clr/BW | 101 | Black & White on / off |
| Before After | 102 | `\` |
| C1 | 49 | Z (zoom) |
| C3–C6 | 51–54 | colour labels red, yellow, green, blue (6–9) |

Not bound: D1 (CC 41), D2 (CC 42), C2, L1–L3, Col, Fn, Tab, Custom Mode, and the
Texture and Dehaze sliders.

## Changing the mapping

Put a `midi.json` in the data folder (`~/Library/Application Support/RAWmakase`
on macOS, `%APPDATA%\RAWmakase` on Windows). It changes the defaults above:

```json
{
  "port": "Loupedeck",
  "photo_dial": 48,
  "photo_detent": 2,
  "dials": { "41": "texture", "42": "dehaze", "33": null },
  "buttons": { "50": "cmd+shift+u", "95": null, "114": "hold:shift" }
}
```

- `port`: part of the MIDI port's name.
- `photo_dial`: the CC that moves between photos, or `null` for none.
  `photo_detent`: its ticks per photo (1–64).
- `dials`: CC number to `exposure`, `contrast`, `highlights`, `shadows`,
  `whites`, `blacks`, `texture`, `clarity`, `dehaze`, `vibrance`, `saturation`,
  `temperature`, `tint` or `band1`–`band8`.
- `buttons`: note number to a key (`"z"`, `"backslash"`, `"cmd+shift+z"`,
  `"alt+arrowleft"`), `"hold:shift"` (a modifier held while the button is down),
  `"mixer:hue"` / `"mixer:sat"` / `"mixer:lum"`, or `"toggle:bw"`.
- `null` removes a default. Other devices work if they send the same kinds of
  message; set `port` and the numbers.

## Mapping another device

Any MIDI monitor shows what a control sends. A dial that sends relative control
changes (1 up, 127 down) maps to a slider under `dials`; a button that sends
`note_on` maps to a key under `buttons`.
