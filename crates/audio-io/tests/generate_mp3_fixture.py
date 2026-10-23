"""Generate our synthetic stereo regression fixture; encoder is a developer-only tool.

Usage: python3 generate_mp3_fixture.py /path/to/ffmpeg
Original tones, MIT OR Apache-2.0. No third-party recordings or encoder binary are bundled.
Reference encoder: FFmpeg 7.1 with libmp3lame.
"""
import math
from pathlib import Path
import struct
import subprocess
import sys
import tempfile
import wave

target = Path(__file__).parent / "fixtures" / "synthetic-stereo.mp3"
target.parent.mkdir(exist_ok=True)
with tempfile.TemporaryDirectory(prefix="sculpt-mp3-fixture-") as temporary:
    source = Path(temporary) / "tones.wav"
    with wave.open(str(source), "wb") as output:
        output.setnchannels(2)
        output.setsampwidth(2)
        output.setframerate(44100)
        samples = bytearray()
        for frame in range(5292):
            samples += struct.pack("<hh", round(13000 * math.sin(frame * 2 * math.pi * 440 / 44100)),
                                   round(7000 * math.sin(frame * 2 * math.pi * 997 / 44100)))
        output.writeframes(samples)
    subprocess.run([sys.argv[1], "-nostdin", "-hide_banner", "-loglevel", "error", "-i", str(source),
                    "-map_metadata", "-1", "-c:a", "libmp3lame", "-b:a", "128k", "-write_xing", "1",
                    "-fflags", "+bitexact", "-flags:a", "+bitexact", "-y", str(target)], check=True)
    subprocess.run([sys.argv[1], "-nostdin", "-hide_banner", "-loglevel", "error", "-i", str(source),
                    "-map_metadata", "-1", "-c:a", "libmp3lame", "-ac", "1", "-ar", "48000",
                    "-b:a", "96k", "-write_xing", "0", "-fflags", "+bitexact", "-flags:a", "+bitexact",
                    "-y", str(target.with_name("synthetic-mono-no-xing.mp3"))], check=True)
