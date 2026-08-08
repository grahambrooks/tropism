# Shared helpers for the evaluation scripts. Sourced, never executed.
# shellcheck shell=bash

log() { printf '%s %s\n' "$(date -u +%H:%M:%S)" "$*" >&2; }

# Free space on the filesystem holding $1, in MiB.
free_mib() {
  df -Pm "$1" 2>/dev/null | awk 'NR==2 {print $4}'
}

human() { # bytes-ish MiB -> human
  awk -v m="$1" 'BEGIN { if (m > 1024) printf "%.1fG", m/1024; else printf "%dM", m }'
}

# Refuses to start a repository that could fill the disk.
#
# The corpus includes kubernetes, vscode, elasticsearch and dotnet/runtime; an
# oracle pass then installs their dependencies on top. Running out of space
# half way through a repository leaves a corrupt checkout that the resume logic
# would happily treat as complete, so this stops first and says so.
disk_guard() {
  local path="$1" need="${2:-$MIN_FREE_MIB}" avail
  avail=$(free_mib "$path")
  [ -z "$avail" ] && return 0
  if [ "$avail" -lt "$need" ]; then
    log "STOP: only $(human "$avail") free under $path, need $(human "$need")"
    log "      free space, or run ./clean.sh, then re-run — completed work is kept"
    return 1
  fi
  return 0
}

# Clone one repository at a pinned SHA, cheaply and without git-lfs.
#
# **LFS is disabled deliberately, not worked around.** microsoft/vscode stores
# fixtures such as `extensions/copilot/test/simulation/cache/base.sqlite` in LFS,
# and a checkout on a machine without `git-lfs` aborts the whole run:
#
#     git-lfs filter-process: git-lfs: command not found
#     fatal: ... smudge filter lfs failed
#
# Overriding the filters leaves those paths as their pointer stubs, which is
# exactly right: they are binary test fixtures, tropism never reads them, and
# fetching them would cost gigabytes across the corpus for no analysis value.
# `required=false` means a filter that fails cannot abort the checkout.
#
# Blobless rather than `--depth 1` alone, because a pinned SHA is usually not the
# branch tip by the time this runs.
fetch_repo() {
  local repo="$1" sha="$2" dir="$3"
  local -a nolfs=(
    -c filter.lfs.smudge=cat
    -c filter.lfs.process=
    -c filter.lfs.required=false
  )

  if [ -d "$dir/.git" ] && [ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" = "$sha" ]; then
    return 0
  fi
  rm -rf "$dir"
  mkdir -p "$dir"

  git -C "$dir" init --quiet
  git -C "$dir" remote add origin "https://github.com/$repo.git"
  git -C "$dir" "${nolfs[@]}" fetch --quiet --depth 1 --filter=blob:none origin "$sha" \
    || return 1
  git -C "$dir" "${nolfs[@]}" checkout --quiet FETCH_HEAD || return 1
}

# Size of a directory in MiB, for the cleanup log.
size_mib() { du -sm "$1" 2>/dev/null | awk '{print $1}'; }
