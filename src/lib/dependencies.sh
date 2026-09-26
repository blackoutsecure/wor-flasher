#!/bin/bash

#Shared dependency declarations consumed by the engine and the native macOS preflight.
#Data only, so every entry point validates or installs exactly the same tools.
WOR_MACOS_BREW_FORMULAE=(aria2 cabextract jq wget wimlib gptfdisk pv)
WOR_LINUX_PACKAGES=(yad aria2 cabextract wimtools chntpw genisoimage exfat-fuse wget udftools bc parted dosfstools unzip git pv)
