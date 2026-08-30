#!/bin/bash

script_dir() {
  if command -v realpath >/dev/null ;then
    realpath "$1"
  else
    (cd "$1" && pwd -P)
  fi
}

: "${PASSED:=0}"
: "${FAILED:=0}"
: "${SKIPPED:=0}"

pass() { printf '  \e[92mPASS\e[0m  %s\n' "$1"; PASSED=$((PASSED+1)); }
fail() { printf '  \e[91mFAIL\e[0m  %s\n' "$1"; FAILED=$((FAILED+1)); }
skip() { printf '  \e[93mSKIP\e[0m  %s\n' "$1"; SKIPPED=$((SKIPPED+1)); }
info() { printf '\e[96m%s\e[0m\n' "$1"; }
die()  { printf '\e[91m%s\e[0m\n' "$1" 1>&2; exit 1; }

summary() {
  echo
  printf 'passed %s, failed %s, skipped %s\n' "$PASSED" "$FAILED" "$SKIPPED"
  [ "$FAILED" -gt 0 ] && exit 1
  exit 0
}