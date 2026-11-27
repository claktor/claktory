#!/usr/bin/env python3
"""Makes the Windows app's icons from the Mac app's, on a Mac.

    python3 app/windows/tool/make_icons.py

Writes app/windows/runner/resources/app_icon.ico and notification_icon.png
(Claktory, from
app/macos/Runner/Assets.xcassets/AppIcon.appiconset/app_icon_1024.png) and
app_icon_dev.ico and notification_icon_dev.png (Claktory Dev, from
app/macos/dev/AppIcon.icns), so the two
apps keep the Mac's two icons (AGENTS.md: the real one a white cog on
graphite, Dev its light inverse). Run it again after `swift scripts/icons.swift`
redraws the Mac's icons, and commit the .ico files.

Each .ico holds PNG images (Windows Vista and later read them) at the sizes
Windows asks for: the taskbar, Start, Explorer's views and Alt+Tab at 100% to
400% scale. Resizing is macOS's own `sips`; `iconutil` unpacks the .icns.
Nothing but the standard library otherwise.
"""

import os
import struct
import subprocess
import sys
import tempfile

SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]

HERE = os.path.dirname(os.path.abspath(__file__))
APP = os.path.normpath(os.path.join(HERE, "..", ".."))
OUT = os.path.join(APP, "windows", "runner", "resources")


def run(*args):
    subprocess.run(args, check=True, stdout=subprocess.DEVNULL)


def resized(source, size, folder):
    path = os.path.join(folder, f"{size}.png")
    run("sips", "-z", str(size), str(size), source, "--out", path)
    with open(path, "rb") as f:
        return f.read()


def write_ico(path, images):
    """images: [(size, png bytes)], smallest first."""
    header = struct.pack("<HHH", 0, 1, len(images))
    offset = len(header) + 16 * len(images)
    entries = b""
    data = b""
    for size, png in images:
        # A width or height of 256 is written as 0.
        side = 0 if size >= 256 else size
        entries += struct.pack(
            "<BBBBHHII", side, side, 0, 0, 1, 32, len(png), offset + len(data)
        )
        data += png
    with open(path, "wb") as f:
        f.write(header + entries + data)


def make(source, name, folder):
    images = [(size, resized(source, size, folder)) for size in SIZES]
    target = os.path.join(OUT, name)
    write_ico(target, images)
    print(f"wrote {os.path.relpath(target, APP)}")
    # The picture Windows shows on the app's notifications (its
    # AppUserModelId registration's IconUri): a PNG beside the app.
    png = os.path.join(OUT, name.replace("app_icon", "notification_icon").replace(".ico", ".png"))
    with open(png, "wb") as f:
        f.write(dict(images)[256])
    print(f"wrote {os.path.relpath(png, APP)}")


def main():
    if sys.platform != "darwin":
        sys.exit("make_icons.py needs macOS (sips, iconutil)")
    real = os.path.join(
        APP, "macos", "Runner", "Assets.xcassets", "AppIcon.appiconset",
        "app_icon_1024.png",
    )
    dev_icns = os.path.join(APP, "macos", "dev", "AppIcon.icns")
    with tempfile.TemporaryDirectory() as folder:
        make(real, "app_icon.ico", folder)
        iconset = os.path.join(folder, "dev.iconset")
        run("iconutil", "-c", "iconset", dev_icns, "-o", iconset)
        largest = max(
            (f for f in os.listdir(iconset) if f.endswith(".png")),
            key=lambda f: os.path.getsize(os.path.join(iconset, f)),
        )
        dev_folder = os.path.join(folder, "dev")
        os.mkdir(dev_folder)
        make(os.path.join(iconset, largest), "app_icon_dev.ico", dev_folder)


if __name__ == "__main__":
    main()
