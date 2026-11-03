"""Regenerates the demo's sound effects (deterministic, Python stdlib only).

    python3 examples/room/tools/make_sounds.py

Writes 22.05 kHz mono 16-bit WAV files to examples/room/assets/sounds/.
Their .meta.json files (asset IDs) are kept, so scenes stay valid.
"""

import math
import random
import struct
import wave
from pathlib import Path

RATE = 22050
OUT = Path(__file__).resolve().parent.parent / "assets" / "sounds"


def write(name, samples):
    OUT.mkdir(parents=True, exist_ok=True)
    peak = max(1e-9, max(abs(s) for s in samples))
    gain = 0.8 / peak
    with wave.open(str(OUT / name), "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(RATE)
        w.writeframes(b"".join(struct.pack("<h", int(s * gain * 32767)) for s in samples))


def hum():
    # Exactly 2 s: 50 Hz and harmonics complete whole cycles, so it loops cleanly.
    n = RATE * 2
    out = []
    for i in range(n):
        t = i / RATE
        s = (0.6 * math.sin(2 * math.pi * 50 * t)
             + 0.3 * math.sin(2 * math.pi * 100 * t)
             + 0.15 * math.sin(2 * math.pi * 150 * t)
             + 0.05 * math.sin(2 * math.pi * 400 * t) * (0.5 + 0.5 * math.sin(2 * math.pi * 2 * t)))
        out.append(s)
    return out


def door():
    rng = random.Random(7)
    n = int(RATE * 1.3)
    out, lp = [], 0.0
    for i in range(n):
        t = i / RATE
        env = min(1.0, t / 0.05) * max(0.0, 1.0 - t / 1.3) ** 0.7
        lp += 0.08 * (rng.uniform(-1, 1) - lp)  # rumbling low-passed noise
        grind = math.sin(2 * math.pi * (70 + 25 * t) * t)
        thud = math.exp(-max(0.0, t - 1.15) * 30) * math.sin(2 * math.pi * 55 * t) if t > 1.15 else 0.0
        out.append(env * (0.7 * lp * 4 + 0.3 * grind) + thud)
    return out


def lever():
    rng = random.Random(3)
    n = int(RATE * 0.35)
    out = []
    for i in range(n):
        t = i / RATE
        click = math.exp(-t * 60) * rng.uniform(-1, 1)
        clunk = math.exp(-t * 18) * math.sin(2 * math.pi * 180 * t)
        out.append(0.6 * click + 0.8 * clunk)
    return out


if __name__ == "__main__":
    write("machine_hum.wav", hum())
    write("door_slide.wav", door())
    write("lever.wav", lever())
    print("wrote", sorted(p.name for p in OUT.glob("*.wav")))
