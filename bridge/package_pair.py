"""Assemble two pinned, tested universal Mac app bundles without altering signed bytes."""
import copy
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import struct
import subprocess
import sys
import time
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parent.parent
DAW_SHA = os.environ["DAW_SHA"]
SHOW_SHA = os.environ["SHOW_SHA"]


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def wait_for_daw():
    run_id = int(os.environ["DAW_RUN_ID"])
    url = f"https://api.github.com/repos/mrgianluca990-bit/SculptDAW/actions/runs/{run_id}"
    headers = {"Authorization": "Bearer " + os.environ["GH_TOKEN"], "Accept": "application/vnd.github+json"}
    deadline = time.monotonic() + 45 * 60
    while time.monotonic() < deadline:
        with urllib.request.urlopen(urllib.request.Request(url, headers=headers), timeout=30) as response:
            run = json.load(response)
        require(run["head_sha"] == DAW_SHA, "DAW workflow does not match the pinned source")
        if run["status"] == "completed":
            require(run["conclusion"] == "success", "DAW build did not pass every gate")
            print("Matching DAW build passed all checks.", flush=True)
            return
        print("Waiting for the matching DAW app build.", flush=True)
        time.sleep(30)
    raise RuntimeError("Timed out waiting for the DAW build")


def universal(header, label):
    formats = {bytes.fromhex("cafebabe"): (">", 20), bytes.fromhex("bebafeca"): ("<", 20),
               bytes.fromhex("cafebabf"): (">", 32), bytes.fromhex("bfbafeca"): ("<", 32)}
    require(header[:4] in formats, f"{label}: expected a universal Mach-O")
    endian, stride = formats[header[:4]]
    require(struct.unpack(endian + "I", header[4:8])[0] == 2, f"{label}: expected two architectures")
    cpus = {struct.unpack(endian + "I", header[8+i*stride:12+i*stride])[0] for i in range(2)}
    require(cpus == {0x01000007, 0x0100000c}, f"{label}: expected Intel and Apple Silicon")


def bundle():
    revision = subprocess.check_output(["git", "-C", str(ROOT / ".bridge-show"), "rev-parse", "HEAD"], text=True).strip()
    require(revision == SHOW_SHA, "Show source differs from the tested revision")
    daw = ROOT / "incoming/daw/Sculpt_DAW_0.4.0_Mac_Universal.zip"
    checksum = daw.with_name(daw.name + ".sha256").read_text().split()[0]
    require(hashlib.sha256(daw.read_bytes()).hexdigest() == checksum, "DAW ZIP checksum mismatch")
    show = ROOT / ".bridge-show/dist/Sculpt-Show-macOS.zip"
    sources = [show, daw]
    expected = {"Sculpt Show.app", "Sculpt DAW.app"}
    originals = {}
    for source in sources:
        with zipfile.ZipFile(source) as archive:
            require(archive.testzip() is None, f"Corrupt ZIP: {source.name}")
            for member in archive.infolist():
                path = Path(member.filename)
                require(not path.is_absolute() and ".." not in path.parts, "Unsafe archive member")
                require(path.parts[0] in expected, "Unexpected top-level archive entry")
                require(member.filename not in originals, "Duplicate archive member")
                originals[member.filename] = (member.file_size, member.CRC, member.external_attr)
                if member.filename == "Sculpt DAW.app/Contents/Info.plist":
                    info = plistlib.loads(archive.read(member))
                    require(info["SoundCraftBuildCommit"] == DAW_SHA, "DAW bundle source mismatch")
    stage = ROOT / "target/paired-verify"
    stage.mkdir(parents=True, exist_ok=True)
    subprocess.run(["ditto", "-x", "-k", str(daw), str(stage)], check=True)
    show_app = ROOT / ".bridge-show/dist/Sculpt Show.app"
    daw_app = stage / "Sculpt DAW.app"
    for app in [show_app, daw_app]:
        subprocess.run(["codesign", "--verify", "--deep", "--strict", "--verbose=2", str(app)], check=True)
    helper = daw_app / "Contents/Helpers/soundcraft-cli"
    subprocess.run(["codesign", "--verify", "--strict", str(helper)], check=True)
    subprocess.run([str(helper), "--version"], check=True)
    destination = ROOT / "dist/paired/Sculpt_Bridge_Mac_Universal.zip"
    destination.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(destination, "w") as combined:
        for source in sources:
            with zipfile.ZipFile(source) as archive:
                for member in archive.infolist():
                    with archive.open(member) as incoming, combined.open(copy.copy(member), "w") as outgoing:
                        shutil.copyfileobj(incoming, outgoing, 1024 * 1024)
    with zipfile.ZipFile(destination) as archive:
        require(archive.testzip() is None, "Combined archive CRC verification failed")
        require({n.split("/")[0] for n in archive.namelist()} == expected, "Expected exactly two app bundles")
        for member in archive.infolist():
            require((member.file_size, member.CRC, member.external_attr) == originals[member.filename],
                    "Signed bytes or permissions changed while combining")
        for binary in ["Sculpt Show.app/Contents/MacOS/SculptShow",
                       "Sculpt DAW.app/Contents/MacOS/sculpt-daw",
                       "Sculpt DAW.app/Contents/Helpers/soundcraft-cli"]:
            require(archive.getinfo(binary).external_attr >> 16 & 0o111 != 0, f"{binary}: missing executable permissions")
            with archive.open(binary) as stream:
                universal(stream.read(80), binary)
    print("Verified paired app ZIP:", destination.name)
    print("SHA256", hashlib.sha256(destination.read_bytes()).hexdigest())


if __name__ == "__main__":
    require(len(sys.argv) == 2 and sys.argv[1] in {"wait", "bundle"}, "Use wait or bundle")
    if sys.argv[1] == "wait":
        wait_for_daw()
    else:
        bundle()
