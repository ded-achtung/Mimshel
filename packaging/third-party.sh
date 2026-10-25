# Third-party sources the installer builds from, pinned to reviewed commits.
# Sourced by install.sh and tools/make-offline.sh.
#
# To update: pick a new upstream tag, review the diff of its install.sh and
# cursors/install.sh, put the tag's commit SHA here and test install.sh on a
# real machine. The installer never moves to a newer commit on its own.

# Colloid icons + cursors, tag 2026-08-10.
COLLOID_ICON_URL=https://github.com/vinceliuice/Colloid-icon-theme.git
COLLOID_ICON_REV=e3a8f89fac888af715233f8f8ef0a5b1e350f0dd

# Colloid GTK/xfwm4 theme, tag 2026-08-08.
COLLOID_GTK_URL=https://github.com/vinceliuice/Colloid-gtk-theme.git
COLLOID_GTK_REV=6c2dc65865628bda9fdc8157a30cd5eda6fd41f9
