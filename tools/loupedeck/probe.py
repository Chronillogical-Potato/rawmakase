#!/usr/bin/env python3
"""Wait for one gesture on the Loupedeck+, report which MIDI signature(s) it sent.

Usage: probe.py LABEL [max_wait_seconds]
Appends {"label":..., "signatures":[...]} to probes.jsonl.
"""
import json
import sys
import time

import rtmidi

label = sys.argv[1]
max_wait = float(sys.argv[2]) if len(sys.argv) > 2 else 30
QUIET = float(sys.argv[3]) if len(sys.argv) > 3 else 1.0

mi = rtmidi.MidiIn()
idx = next(i for i, p in enumerate(mi.get_ports()) if "Loupedeck" in p)
mi.open_port(idx)
mi.ignore_types(sysex=False, timing=False, active_sense=False)
while mi.get_message():  # drop anything already queued
    pass

msgs, start, last = [], time.time(), None
while True:
    m = mi.get_message()
    now = time.time()
    if m:
        msgs.append(m[0])
        last = now
    elif last is None and now - start > max_wait:
        break
    elif last is not None and now - last > QUIET:
        break
    else:
        time.sleep(0.002)

KINDS = {0x80: "note_off", 0x90: "note_on", 0xB0: "cc", 0xE0: "pitchbend", 0xA0: "poly_at", 0xD0: "chan_at", 0xC0: "prog"}
groups = {}
order = []
for msg in msgs:
    kind, ch = msg[0] & 0xF0, (msg[0] & 0x0F) + 1
    num = None if kind == 0xE0 else msg[1]
    key = (KINDS.get(kind, hex(kind)), ch, num)
    if key not in groups:
        order.append(f"{key[0]}#{key[2]}")
    groups.setdefault(key, []).append(msg)
sigs = []
for (kind, ch, num), ms in groups.items():
    if kind == "pitchbend":
        vals = [m[1] | (m[2] << 7) for m in ms]
    else:
        vals = [m[2] for m in ms] if len(ms[0]) > 2 else []
    sigs.append({"kind": kind, "channel": ch, "number": num, "count": len(ms),
                 "values": sorted(set(vals))[:6], "min": min(vals, default=None), "max": max(vals, default=None)})
with open("probes.jsonl", "a") as f:
    f.write(json.dumps({"label": label, "order": order, "signatures": sigs}) + "\n")
print(f"{label}: order={order}\n" + ("NOTHING received" if not sigs else json.dumps(sigs)))
