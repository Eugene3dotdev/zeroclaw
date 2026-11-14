#!/usr/bin/env bash
# Guard for the fleet decision (Evgenii Tretiakov, blaze switch): Windows
# targets are dropped. No workflow may run on a Windows runner or build a
# *-windows-* target, and .cargo/config.toml carries no Windows target table.
set -u
root="${1:-.}"
cargo_cfg="$root/.cargo/config.toml"
workflows=()
for f in "$root"/.github/workflows/*.yml "$root"/.github/workflows/*.yaml; do
  # An unmatched glob stays literal; skip it so a repo with no *.yaml is fine.
  [ -e "$f" ] && workflows+=("$f")
done
# A missing checkout must fail, not read as "no Windows target found".
if [ ! -r "$cargo_cfg" ] || [ "${#workflows[@]}" -eq 0 ]; then
  printf 'no_windows_targets: cannot read %s or any workflow under %s/.github/workflows\n' "$cargo_cfg" "$root" >&2
  exit 2
fi
# grep exits 1 when nothing matches and 2 when it cannot read a file.
hits=$(grep -HnE 'windows-(latest|20[0-9]{2}|[0-9]+-arm)|[a-z0-9_]+-pc-windows-(msvc|gnu)' "${workflows[@]}")
hits_rc=$?
cfg=$(grep -nE '^\[target\.[^]]*windows' "$cargo_cfg")
cfg_rc=$?
if [ "$hits_rc" -gt 1 ] || [ "$cfg_rc" -gt 1 ]; then
  echo "no_windows_targets: grep could not read its inputs" >&2
  exit 2
fi
if [ -n "$hits$cfg" ]; then
  printf 'no_windows_targets: Windows target still present:\n%s\n%s\n' "$hits" "$cfg" >&2
  exit 1
fi
echo "no_windows_targets: ok"
