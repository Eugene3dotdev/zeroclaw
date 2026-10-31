#!/usr/bin/env bash
# Guard for the fleet decision (Evgenii Tretiakov, blaze switch): Windows
# targets are dropped. No workflow may run on a Windows runner or build a
# *-windows-* target, and .cargo/config.toml carries no Windows target table.
set -u
root="${1:-.}"
hits=$(grep -nE 'windows-(latest|20[0-9]{2})|[a-z0-9_]+-pc-windows-(msvc|gnu)' "$root"/.github/workflows/*.yml "$root"/.github/workflows/*.yaml 2>/dev/null)
cfg=$(grep -nE '^\[target\.[^]]*windows' "$root/.cargo/config.toml" 2>/dev/null)
if [ -n "$hits$cfg" ]; then
  printf 'no_windows_targets: Windows target still present:\n%s\n%s\n' "$hits" "$cfg" >&2
  exit 1
fi
echo "no_windows_targets: ok"
