#!/usr/bin/env python3
"""Capture raw MIDI from the Loupedeck+ and log it (stdout + JSONL file).

Usage: .venv/bin/python capture.py [seconds]   (default: until Ctrl-C)
Press / turn every control once, noting which one, to build the mapping table.
"""
import json
import sys
import time

import rtmidi

LOG = "capture.jsonl"
NAMES = {0x80: "note_off", 0x90: "note_on", 0xA0: "poly_aftertouch",
         0xB0: "control_change", 0xC0: "program_change",
         0xD0: "channel_aftertouch", 0xE0: "pitch_bend"}


def decode(msg):
    status = msg[0]
    if status >= 0xF0:
        return {"type": "system", "status": status, "data": msg[1:]}
    kind = NAMES.get(status & 0xF0, "unknown")
    return {"type": kind, "channel": (status & 0x0F) + 1, "data": msg[1:]}


def main():
    duration = float(sys.argv[1]) if len(sys.argv) > 1 else None
    midi_in = rtmidi.MidiIn()
    ports = midi_in.get_ports()
    idx = next((i for i, p in enumerate(ports) if "Loupedeck" in p), None)
    if idx is None:
        sys.exit(f"Loupedeck MIDI port not found. Ports: {ports}")
    midi_in.open_port(idx)
    midi_in.ignore_types(sysex=False, timing=False, active_sense=False)
    print(f"Listening on {ports[idx]!r} - press/turn controls (Ctrl-C to stop)")

    start = time.time()
    with open(LOG, "a") as f:
        try:
            while duration is None or time.time() - start < duration:
                m = midi_in.get_message()
                if m is None:
                    time.sleep(0.001)
                    continue
                msg, _ = m
                ev = {"t": round(time.time() - start, 3), "raw": msg, **decode(msg)}
                f.write(json.dumps(ev) + "\n")
                f.flush()
                print(f"{ev['t']:8.3f}  {ev['type']:<15} ch={ev.get('channel', '-')}  "
                      f"data={ev['data']}  raw={[hex(b) for b in msg]}")
        except KeyboardInterrupt:
            pass
    midi_in.close_port()


if __name__ == "__main__":
    main()
