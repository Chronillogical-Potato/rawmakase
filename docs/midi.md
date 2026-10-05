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
| Up / Left, Down / Right | 76 / 78, 77 / 79 | previous / next photo |
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
Texture and Dehaze sliders. [tools/loupedeck/controls.json](../tools/loupedeck/controls.json)
lists every control the device sends.

## Changing the mapping in Preferences

Preferences > Automation lists every dial, fader and button the Loupedeck+
sends, each with the action it runs. Pick another from the list, or type an action name or supported shortcut
(`cmd+shift+z`, `hold:shift`, `toggle:bw`) into the field beside a button. Changes
apply at once and are saved to `midi.json` (device mappings) and `automation.json` (external control), as described below, so
only what you changed is written. The page also shows whether the device is
connected and the last message it sent, and Restore Default Actions undoes every
change to the dials and buttons.

The same page has a **External control** checkbox: with it off, RAWmakase does not
listen for `rawmakase-ctl`, and `control.json` is removed. Turning it on listens
again on a new port with a new token.

## Changing the mapping in midi.json

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

## Controlling it from a script

Use the built-in `rawmakase control` command or the optional `rawmakase-ctl`
client. Enable external control in Preferences first; it is off by default.
See [External control](automation.md) for commands, explicit photo/mask targets,
preview/export completion, the versioned protocol and timeout behavior.

```sh
rawmakase control capabilities
rawmakase control open --id 17
rawmakase control set exposure 0.5
rawmakase control action undo
rawmakase control preview /absolute/path/preview.jpg
```

`dial` and `press` retain the Loupedeck names and use the configured MIDI mapping.
`set`, `turn` and named `action` commands use application operations independently
of device mappings. Supported legacy key names also resolve to those operations;
they do not inject keyboard events.

In Masking, device dials adjust the selected mask where a local parameter exists.
Unsupported local parameters are rejected instead of changing the global edit.
Outside Masking they adjust the global controls. No dials edit while a modal or
blocking operation is active. Semantic scripts use global scope by default and
can explicitly address masks with generation/revision guards.

## Mapping another device

[tools/loupedeck](../tools/loupedeck) has the scripts used to map the Loupedeck+:

```bash
python3 -m venv .venv && .venv/bin/pip install -r tools/loupedeck/requirements.txt
.venv/bin/python tools/loupedeck/capture.py     # print every message
.venv/bin/python tools/loupedeck/mapper.py      # operate a control, name it
.venv/bin/python tools/loupedeck/probe.py "Fader P1"   # report one gesture
```

They write `capture.jsonl`, `mapping.json` and `probes.jsonl` to the current
directory.

The built-in profile uses Loupedeck+ names. Preferences also lists configured
controls and the last received unknown CC/note so other device numbers can be
mapped. The MIDI backend is available on macOS and Windows; it currently accepts
the relative encoder encoding documented above, not absolute-position faders.
