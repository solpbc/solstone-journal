#!/usr/bin/env bash
# Populate journal/chronicle/ with media from a local clone of
# github.com/solpbc/field_journal — a public-domain corpus for dev/test runs.
#
# Opt-in dev primitive. Not part of the canonical install/setup paths; run only
# when you want a contributor/integration-test journal seeded from public media
# rather than personal capture data. Full guide: docs/FIELD_JOURNAL.md.
#
# Usage:
#   ./setup_field_journal.sh [--source PATH] [--force]
#
# Options:
#   --source PATH  Path to the field_journal clone. Default: ~/Field_Journal
#   --force        Overwrite existing chronicle days
#   -h, --help     Show this help
#
# The script copies (does not symlink) each YYYYMMDD day directory from
# <source>/journal/ into ./journal/chronicle/. Copying is intentional: solstone
# writes derived artifacts (audio.jsonl, audio.npz, descriptions, etc.) as
# siblings of source media, so symlinking would dirty the field_journal clone.
#
# Run `sol setup` afterward to bring the journal to a ready-to-process state.

set -euo pipefail

SOURCE="${HOME}/Field_Journal"
FORCE=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --source)
            SOURCE="$2"
            shift 2
            ;;
        --force)
            FORCE=1
            shift
            ;;
        -h|--help)
            sed -n '2,/^$/p' "$0" | sed 's/^# \?//'
            exit 0
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

REPO_ROOT="$(cd "$(dirname "$0")" && pwd)"
DEST="${REPO_ROOT}/journal/chronicle"

if [[ ! -d "${SOURCE}/journal" ]]; then
    echo "field_journal not found at ${SOURCE}/journal" >&2
    echo "Clone it first: git clone https://github.com/solpbc/field_journal ${SOURCE}" >&2
    exit 1
fi

if [[ ! -d "${REPO_ROOT}/journal" ]]; then
    echo "solstone journal/ missing at ${REPO_ROOT}/journal" >&2
    echo "Run 'make install' and bootstrap your journal before populating it." >&2
    exit 1
fi

mkdir -p "${DEST}"

copied=0
skipped=0
overwrote=0

for day_path in "${SOURCE}/journal/"[0-9]*/; do
    day="$(basename "${day_path}")"
    if [[ ! "${day}" =~ ^[0-9]{8}$ ]]; then
        continue
    fi
    target="${DEST}/${day}"
    if [[ -e "${target}" ]]; then
        if [[ "${FORCE}" -eq 1 ]]; then
            rm -r "${target}"
            cp -a "${day_path%/}" "${target}"
            overwrote=$((overwrote + 1))
            echo "  [overwrite] ${day}"
        else
            skipped=$((skipped + 1))
            echo "  [skip]      ${day} (already present — use --force to replace)"
        fi
    else
        cp -a "${day_path%/}" "${target}"
        copied=$((copied + 1))
        echo "  [copy]      ${day}"
    fi
done

echo
echo "Done. ${copied} copied, ${overwrote} overwritten, ${skipped} skipped."
echo "Chronicle now at: ${DEST}"
