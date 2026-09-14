#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (c) 2026 sol pbc

# Uploads a locally-staged release tree to the release origin,
# https://updates.solstone.app/solstone-journal/{lane}/{version}/{filename},
# mirrors CHANGELOG.md, and advances each lane's `latest` pointer only after
# every artifact under it is fully published.
#
# This is the upload half of what solstone-linux's and solstone-tmux's
# scripts/packaging/publish-origin.sh do in one step. Journal's own
# `solstone-distribution publish --dest <stage>` subcommand already does the
# OTHER half — signature verification against the manifest and laying the
# candidate out locally at exactly this key layout — so this script does not
# re-verify a signature or re-derive a version; it uploads what is already on
# disk, at the paths `publish` already computed, and nothing else.
#
# Typical use (unchanged from the existing manual procedure, now one command
# instead of a hand-run wrangler sequence):
#
#   cargo run --locked --manifest-path core/Cargo.toml -p solstone-core-distribution \
#     --bin solstone-distribution -- publish --lane release "$CANDIDATE_DIR" --dest "$STAGE"
#   scripts/publish-origin-upload.sh --stage "$STAGE" --changelog CHANGELOG.md
#
# Multiple `publish` invocations (one per target: macos-arm64, linux-x86_64,
# linux-aarch64...) may write into the same $STAGE before one upload call —
# this script discovers every object under $STAGE/solstone-journal/ rather
# than taking an explicit lane/version, so it uploads whatever is actually
# there without a second, independently-typed description of it to drift out
# of sync with the tree.
#
# Usage: publish-origin-upload.sh --stage <dir> [--changelog <file>] [--dry-run]
#
# --changelog is optional and orthogonal to --stage: pass it only when this
# upload is publishing a real, HEAD-bound release lane cut and you want the
# origin's copy of CHANGELOG.md to move — the script does not infer this from
# lane names because a stage tree can legitimately mix lanes.

set -euo pipefail

umask 077
export LC_ALL=C

PRODUCT="solstone-journal"
BUCKET="${SOLSTONE_ORIGIN_BUCKET:-solstone-updates}"
ORIGIN_URL="https://updates.solstone.app"

die() {
    printf 'release origin uploader: %s\n' "$1" >&2
    exit 1
}

usage() {
    echo "usage: publish-origin-upload.sh --stage <dir> [--changelog <file>] [--dry-run]" >&2
    exit 2
}

stage=""
changelog_file=""
dry_run=false
while (($# > 0)); do
    case "$1" in
        --stage)
            (($# >= 2)) || usage
            stage="$2"
            shift 2
            ;;
        --changelog)
            (($# >= 2)) || usage
            changelog_file="$2"
            shift 2
            ;;
        --dry-run)
            dry_run=true
            shift
            ;;
        *)
            usage
            ;;
    esac
done
[[ -n "$stage" ]] || usage

required_tools=(find realpath rm sort)
$dry_run || required_tools+=(wrangler)
for tool in "${required_tools[@]}"; do
    command -v "$tool" >/dev/null 2>&1 ||
        die "required release tool is unavailable: $tool"
done

[[ -d "$stage" && ! -L "$stage" ]] || die "stage directory must be a real directory"
stage="$(realpath "$stage")"
product_root="$stage/$PRODUCT"
[[ -d "$product_root" ]] || die "stage-empty: no $PRODUCT/ under $stage — nothing to upload"

if [[ -n "$changelog_file" ]]; then
    [[ -f "$changelog_file" && ! -L "$changelog_file" ]] ||
        die "changelog-missing: $changelog_file must be a regular file"
    changelog_file="$(realpath "$changelog_file")"
fi

content_type_for() {
    case "$1" in
        *.tar.gz) echo "application/gzip" ;;
        *.deb) echo "application/vnd.debian.binary-package" ;;
        *.rpm) echo "application/x-rpm" ;;
        *.pkg) echo "application/octet-stream" ;;
        *.json) echo "application/json" ;;
        *.minisig | *.sha256 | *.release | CHANGELOG.md) echo "text/plain; charset=utf-8" ;;
        *) echo "application/octet-stream" ;;
    esac
}

# Dot-separated numeric segments; non-numeric segments compare as strings.
# Same ordering solstone-linux's and solstone-tmux's publishers use, so
# `latest` advances identically across all three products.
version_is_not_older() {
    local left="$1" right="$2"
    local -a left_parts right_parts
    IFS='.' read -r -a left_parts <<<"$left"
    IFS='.' read -r -a right_parts <<<"$right"
    local count=${#left_parts[@]}
    ((${#right_parts[@]} > count)) && count=${#right_parts[@]}
    local index l r
    for ((index = 0; index < count; index++)); do
        l="${left_parts[index]:-}"
        r="${right_parts[index]:-}"
        [[ "$l" == "$r" ]] && continue
        [[ -z "$l" ]] && return 1
        [[ -z "$r" ]] && return 0
        if [[ "$l" =~ ^[0-9]+$ && "$r" =~ ^[0-9]+$ ]]; then
            ((10#$l > 10#$r)) && return 0
            return 1
        fi
        [[ "$l" > "$r" ]] && return 0
        return 1
    done
    return 0
}

stage_scratch="$(mktemp -d "${TMPDIR:-/tmp}/$PRODUCT-publish-origin-upload.XXXXXX")"
cleanup() {
    rm -rf -- "$stage_scratch"
}
trap cleanup EXIT

# Returns 0 when the object is present (bytes land in $2), 1 when it is
# genuinely absent, and fails closed on every other outcome. An unreachable
# origin must never read as an empty one.
remote_get() {
    local key="$1" destination="$2" log status
    log="$stage_scratch/wrangler.log"
    set +e
    wrangler r2 object get "$BUCKET/$key" --remote --file "$destination" >"$log" 2>&1
    status=$?
    set -e
    if ((status == 0)); then
        return 0
    fi
    if grep -qF 'The specified key does not exist' "$log"; then
        rm -f "$destination"
        return 1
    fi
    cat "$log" >&2
    die "origin-unreachable: could not read $key"
}

remote_put() {
    local key="$1" file="$2" content_type="$3" cache_control="$4"
    wrangler r2 object put "$BUCKET/$key" \
        --file "$file" \
        --content-type "$content_type" \
        --cache-control "$cache_control" \
        --remote >/dev/null ||
        die "origin-unreachable: could not write $key"
}

checkpoint() {
    [[ "${SOLSTONE_ORIGIN_FAIL_AFTER:-}" == "$1" ]] || return 0
    die "injected-failure $1"
}

printf 'uploading staged %s tree from %s to %s\n' "$PRODUCT" "$stage" "$ORIGIN_URL"

# Discover every object `solstone-distribution publish` already laid out,
# grouped by lane. A "latest" file is deferred and handled after every other
# object in its lane, never uploaded in this first pass.
declare -A lane_seen=()
non_latest_keys=()
latest_keys=()
while IFS= read -r -d '' path; do
    relative="${path#"$stage"/}"
    if [[ "$relative" == */latest ]]; then
        latest_keys+=("$relative")
    else
        non_latest_keys+=("$relative")
    fi
    lane="$(cut -d/ -f2 <<<"$relative")"
    lane_seen["$lane"]=1
done < <(find "$product_root" -type f -print0 | sort -z)

((${#non_latest_keys[@]} > 0 || ${#latest_keys[@]} > 0)) ||
    die "stage-empty: $product_root contains no files"

for lane in "${!lane_seen[@]}"; do
    case "$lane" in
        release | staging | dev) ;;
        *) die "lane-invalid: unrecognized lane '$lane' under $product_root" ;;
    esac
done

published=()
for relative in "${non_latest_keys[@]}"; do
    local_file="$stage/$relative"
    lane="$(cut -d/ -f2 <<<"$relative")"
    name="${relative##*/}"
    if $dry_run; then
        printf '  would publish %s/%s\n' "$ORIGIN_URL" "$relative"
        continue
    fi
    remote_file="$stage_scratch/remote-object"
    if remote_get "$relative" "$remote_file"; then
        if cmp -s "$remote_file" "$local_file"; then
            printf '  present  %s\n' "$relative"
            rm -f "$remote_file"
            checkpoint "object:$name"
            continue
        fi
        # release and staging versioned objects are immutable, same rule as
        # solstone-linux/solstone-tmux. No R2 bucket lock rule covers these
        # prefixes, so the store will not refuse for us — the refusal is ours.
        [[ "$lane" == "dev" ]] ||
            die "object-immutable: $relative already exists with different bytes"
        rm -f "$remote_file"
    fi
    remote_put "$relative" "$local_file" "$(content_type_for "$name")" \
        "$([[ "$lane" == "dev" ]] && echo "no-cache" || echo "public, max-age=31536000, immutable")"
    printf '  put      %s\n' "$relative"
    published+=("$relative")
    checkpoint "object:$name"
done
checkpoint "objects"

# CHANGELOG.md mirror — lane-independent (there is one changelog, not one per
# lane), uploaded only when the caller passes --changelog. This lets
# solstone.app's release-notes pages read prose release notes straight from
# the origin instead of depending on GitHub Releases for them; the origin
# never carried prose before this.
if [[ -n "$changelog_file" ]]; then
    changelog_key="$PRODUCT/CHANGELOG.md"
    if $dry_run; then
        printf '  would publish %s/%s\n' "$ORIGIN_URL" "$changelog_key"
    else
        changelog_remote="$stage_scratch/remote-changelog"
        if remote_get "$changelog_key" "$changelog_remote" && cmp -s "$changelog_remote" "$changelog_file"; then
            printf '  present  %s\n' "$changelog_key"
        else
            remote_put "$changelog_key" "$changelog_file" "text/plain; charset=utf-8" "no-cache"
            printf '  put      %s\n' "$changelog_key"
        fi
        rm -f "$changelog_remote"
    fi
    checkpoint "changelog"
fi

if $dry_run; then
    for relative in "${latest_keys[@]}"; do
        printf '  would advance %s/%s\n' "$ORIGIN_URL" "$relative"
    done
    exit 0
fi

for relative in "${latest_keys[@]}"; do
    lane="$(cut -d/ -f2 <<<"$relative")"
    local_body="$(cat "$stage/$relative")"
    [[ "$local_body" =~ ^version=[^[:space:]/]+$ ]] ||
        die "latest-invalid: staged $relative is not a single version= line"
    incoming_version="${local_body#version=}"

    latest_remote="$stage_scratch/remote-latest"
    advance=true
    if remote_get "$relative" "$latest_remote"; then
        existing_body="$(cat "$latest_remote")"
        [[ "$existing_body" =~ ^version=[^[:space:]/]+$ ]] ||
            die "latest-invalid: remote $relative is not a single version= line"
        existing_version="${existing_body#version=}"
        if ! version_is_not_older "$incoming_version" "$existing_version"; then
            advance=false
        fi
    fi

    if $advance; then
        remote_put "$relative" "$stage/$relative" "text/plain; charset=utf-8" "no-cache"
        printf '  put      %s (version=%s)\n' "$relative" "$incoming_version"
    else
        printf '  held     %s (already at version=%s)\n' "$relative" "$existing_version"
    fi
    rm -f "$latest_remote"
done

printf 'uploaded %s (%d new objects)\n' "$PRODUCT" "${#published[@]}"
