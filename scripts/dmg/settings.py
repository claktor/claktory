# dmgbuild's settings for the .dmg's window (scripts/make-dmg.sh): the app and
# a link to Applications side by side on scripts/dmg-background.swift's
# background, whose arrow runs between them. make-dmg.sh passes the app's path
# and this folder's as -D app=PATH -D dir=PATH.
import os.path

here = defines["dir"]  # noqa: F821 (dmgbuild's)

format = "ULFO"  # lzfse, macOS 10.11 and later
filesystem = "HFS+"
files = [(defines["app"], "Claktory.app")]  # noqa: F821 (dmgbuild's)
symlinks = {"Applications": "/Applications"}
# background@2x.png beside it makes it sharp on Retina screens.
background = os.path.join(here, "background.png")
window_rect = ((200, 160), (660, 400))
default_view = "icon-view"
show_status_bar = False
show_tab_view = False
show_toolbar = False
show_pathbar = False
show_sidebar = False
icon_size = 128
text_size = 13
icon_locations = {"Claktory.app": (165, 190), "Applications": (495, 190)}
