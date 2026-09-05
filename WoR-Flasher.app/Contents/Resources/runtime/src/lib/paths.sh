#!/bin/bash

#Path resolution shared by every entry point.
#Resolves a path using whichever native tool the host provides, without changing the caller's directory.
resolve_path() { #Input: path. Output: absolute path.
  [ -z "$1" ] && return 1
  if command -v realpath >/dev/null ;then
    realpath "$1" && return 0
  fi
  if readlink -f "$1" >/dev/null 2>&1 ;then
    readlink -f "$1" && return 0
  fi
  if [ -d "$1" ];then
    (cd "$1" && pwd -P)
  else
    local dir
    local base
    dir="$(dirname "$1")"
    base="$(basename "$1")"
    printf '%s/%s\n' "$(cd "$dir" && pwd -P)" "$base"
  fi
}
