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
Texture and Dehaze sliders. [tools/loupedeck/controls.json](../tools/loupedeck/controls.json)
lists every control the device sends.

## Changing the mapping in Preferences

Preferences > Automation lists every dial, fader and button the Loupedeck+
sends, each with the action it runs. Pick another from the list, or type a key
(`cmd+shift+z`, `hold:shift`, `toggle:bw`) into the field beside a button. Changes
apply at once and are saved to `midi.json`, as the next section describes, so
only what you changed is written. The page also shows whether the device is
connected and the last message it sent, and Restore Default Actions undoes every
change to the dials and buttons.

The same page has a **Command line** checkbox: with it off, RAWmakase does not
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

## Controlling it from a script (`rawmakase-ctl`)

The same controls are available from the command line, on every platform.
RAWmakase listens on a loopback TCP port and writes `control.json` (the port and
a random token, readable only by you) in the data folder; the command-line tool
reads it. Nothing leaves the machine, and a program that cannot read
`control.json` cannot send a command. Set `"socket": false` in `midi.json` to
turn it off. The code is in
[src/app/control_surface/socket.rs](../src/app/control_surface/socket.rs); the
tool is a separate crate that is not part of the app's build.

```bash
cargo install --path tools/rawmakase-ctl      # or build it in tools/rawmakase-ctl
rawmakase-ctl state                           # mode, open photo, every slider (JSON)
rawmakase-ctl get exposure
rawmakase-ctl set exposure 0.5                # EV; temperature in kelvin; else -100..100
rawmakase-ctl set band3.sat -20               # band1 (Red) .. band8; .hue .sat .lum .gray
rawmakase-ctl turn contrast 5                 # ticks, like a dial
rawmakase-ctl dial "Fader P3" -4              # a Loupedeck dial by name or CC number
rawmakase-ctl press P7                        # a Loupedeck button by name or note number
rawmakase-ctl press shift --down              # hold a modifier ... then --up
rawmakase-ctl key cmd+shift+z                 # a key, as the keyboard would
rawmakase-ctl mixer sat                       # what the band faders then turn
rawmakase-ctl bw                              # Black & White on / off
rawmakase-ctl photo next                      # or prev
rawmakase-ctl search "DSCF04"                 # the Library's search box; no text clears it
rawmakase-ctl open DSCF0042                   # open a photo in Develop by name
rawmakase-ctl open --id 17                    # ... or by catalog id
rawmakase-ctl develop                         # Develop on the selected (or first) photo
rawmakase-ctl library                         # back to the grid
rawmakase-ctl controls                        # the Loupedeck's names
rawmakase-ctl --state set tint 10             # print the state after any command
```

The tool finds the app's data folder as the app does (`RAWMAKASE_DATA_DIR`, or
`--data-dir`). It exits with 1 and a message on stderr when a command fails.

- `dial` and `press` go through the same mapping as the device, so a `midi.json`
  applies to them; `turn`, `set` and `key` do not depend on it.
- A command is answered once the app has drawn a frame that handled it, with the
  state after it. A minimized or fully hidden window draws no frames: the tool
  then fails after three seconds, and the command is still carried out when the
  window comes back.
- `open` takes a filename in any case, with or without its extension, or a path
  (any part of it, if it contains a `/`); failing that, a part of a filename that
  only one photo has. A master wins over its virtual copies. When several photos
  match, nothing opens and the error lists them with their ids for `--id`. A photo
  hidden by the Library's filters is shown first, as clicking it elsewhere would.
  `open` returns once the photo has loaded (`--no-wait` returns at once, and
  `--timeout` sets how long to wait, 30 s). Edits sent to a photo that is still
  loading are dropped, hence the wait. Photos the app refuses to open in Develop
  fail with the reason the app gives.
- `search` uses the same box and rules as the Library's own search (filename,
  keywords, capture date, label) and switches the filters on if Cmd+L had turned
  them off. It does not change the module.
- Sliders change only with a photo open in Develop. `set` makes a History step of
  its own; `turn` and `dial` merge as a dial does.
- `photo next` moves exactly one photo, even right after another; the photo loads
  after the reply.
- The request format is one line of JSON each way, if you would rather not use the
  tool: `{"token": "...", "cmd": "turn", "param": "exposure", "ticks": 5}`, answered
  by `{"ok": true, "state": {...}}`. Commands: `state`, `cc`, `note`, `turn`, `set`,
  `action`, `photo`, `open` (`name` or `id`), `search` (`text`) and `module`
  (`develop` or `library`). The state has `mode`, `photo`, `photo_id`, `loaded`, the
  slider `values`, and for Library commands a `result` with `ok` and an `error`
  or what was opened.

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
