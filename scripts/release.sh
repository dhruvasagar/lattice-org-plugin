#!/usr/bin/env bash
# Cut a release of the org plugin: bump the version, promote the changelog,
# commit, tag. Nothing is pushed.
#
#   scripts/release.sh                  # last release + what each bump gives
#   scripts/release.sh patch            # 0.1.0 -> 0.1.1
#   scripts/release.sh minor            # 0.1.0 -> 0.2.0
#   scripts/release.sh major            # 0.1.0 -> 1.0.0
#   scripts/release.sh 0.3.0            # an explicit version
#   scripts/release.sh minor --dry-run  # print every step, change nothing
#
# A release IS the tag. Pushing it is what publishes: a user who builds pins
# `rev: Some("vX.Y.Z")` and the plugin manager builds that commit, and the
# `release` workflow runs CI against the tagged commit and, only if it passes,
# lattice's reusable `plugin-release.yml` builds the component and attaches
# it — `org-X.Y.Z-api<M>.<m>.wasm` and its `.sha256` — to a GitHub release,
# for a user who `require_release`s it without a Rust toolchain. The tag is the whole contract, which is why this script guards
# it and nothing else.
#
# The version lives in four places, and this script owns all of them:
#
#   Cargo.toml   the package version
#   Cargo.lock   this package's own entry (rewritten in place: no
#                `cargo update`, so a release never moves a dependency and
#                never needs the network)
#   README.md    the `| Version |` row, the `rev:` pin in the `require`
#                example and the `version:` pin in the `require_release`
#                one — so the snippet a user copies installs this release
#   CHANGELOG.md `## Unreleased` retitled `## X.Y.Z — <date>`
#
# Each rewrite is checked afterwards. A README edited so a pattern no longer
# matches makes the release FAIL rather than ship a page naming the old
# version.
#
# The changelog section also records the plugin API the release builds
# against, read from the locked `lattice-wit` — a plugin version means
# nothing without the editor it runs on, and a line written by hand is a line
# that goes stale.
#
# Bumps count from the LAST RELEASE — the highest `vX.Y.Z` tag — not from
# `Cargo.toml`. The two differ when `Cargo.toml` was bumped ahead of a
# release, and counting from `Cargo.toml` then skips a version (lattice went
# 0.9.9 -> 0.9.11 that way). A `Cargo.toml` already at the bump's result is
# released as is; one already PAST it is refused rather than lowered.
#
# The first release tags the version the crate already carries: with no
# `vX.Y.Z` tag for it, `scripts/release.sh X.Y.Z` (the current version)
# tags it without a bump.
set -euo pipefail
unset CDPATH
cd "$(dirname "$0")/.." >/dev/null

STUB_MARKER="<!-- write the release notes here, then re-run scripts/release.sh -->"

die() { printf '\nrelease: %s\n\n' "$*" >&2; exit 1; }
say() { printf '\n=== %s\n' "$1"; }

read_version() {
    grep -m1 -E '^version[[:space:]]*=' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/'
}

# The plugin API this tree builds against.
#
# `wit/` is not the place to read it: it is gitignored, written by `build.rs`
# from the pinned `lattice-wit` crate, and absent from a fresh clone. The pin
# itself is the answer — the locked `lattice-wit` version. Lattice's own
# guard holds that crate's `major.minor` equal to the WIT package's, so
# `lattice-wit 0.3.0` means `lattice:plugin-host@0.3.x`.
read_api() {
    local v
    v="$(grep -A1 -x 'name = "lattice-wit"' Cargo.lock | sed -n -E 's/^version = "([^"]+)"$/\1/p')"
    [[ "$v" =~ ^([0-9]+)\.([0-9]+)\.[0-9]+$ ]] \
        || die "cannot read the locked lattice-wit version out of Cargo.lock"
    printf '`lattice:plugin-host@%s.%s.x` (lattice-wit %s)\n' \
        "${BASH_REMATCH[1]}" "${BASH_REMATCH[2]}" "$v"
}

extract_section() {
    awk -v ver="$1" '
        BEGIN { gsub(/\./, "\\.", ver) }
        $0 ~ "^## " ver "([ \t]|$)" { inside = 1; print; next }
        inside && /^## / { exit }
        inside { print }
    ' CHANGELOG.md
}

# Rewrite a file through awk, in place.
rewrite() {
    local file="$1"; shift
    local tmp
    tmp="$(mktemp)"
    awk "$@" "$file" > "$tmp"
    cat "$tmp" > "$file"
    rm -f "$tmp"
}

current="$(read_version)"
[[ "$current" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    || die "cannot parse version '$current' out of Cargo.toml"
api="$(read_api)"

# The highest released version: the greatest `vX.Y.Z` tag. Empty before the
# first release, when bumps count from `Cargo.toml` instead.
last_release() {
    git tag --list 'v[0-9]*.[0-9]*.[0-9]*' \
        | sed 's/^v//' \
        | grep -E '^[0-9]+\.[0-9]+\.[0-9]+$' \
        | sort -V | tail -1 || true
}
# Whether version $1 sorts strictly before $2.
version_lt() {
    [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -1)" = "$1" ]
}

released="$(last_release)"
base="${released:-$current}"
[[ "$base" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || die "cannot parse version '$base'"
base_major="${BASH_REMATCH[1]}"
base_minor="${BASH_REMATCH[2]}"
base_patch="${BASH_REMATCH[3]}"

if [ "$#" -eq 0 ]; then
    if git rev-parse -q --verify "refs/tags/v$current" >/dev/null; then
        tagged="tagged"
    else
        tagged="NOT tagged yet — scripts/release.sh $current tags it"
    fi
    cat <<USAGE

  released  v${released:-none}
  Cargo     v$current ($tagged)
  builds    against $api

  patch     v$base_major.$base_minor.$((base_patch + 1))
  minor     v$base_major.$((base_minor + 1)).0
  major     v$((base_major + 1)).0.0

  scripts/release.sh <patch|minor|major|X.Y.Z> [--dry-run]

USAGE
    exit 0
fi

bump=""
dry_run=0
for arg in "$@"; do
    case "$arg" in
        --dry-run)            dry_run=1 ;;
        patch|minor|major)    bump="$arg" ;;
        [0-9]*.[0-9]*.[0-9]*) bump="$arg" ;;
        *) die "unknown argument '$arg' (expected patch|minor|major|X.Y.Z|--dry-run)" ;;
    esac
done
[ -n "$bump" ] || die "no bump given (patch|minor|major|X.Y.Z)"

case "$bump" in
    patch) next="$base_major.$base_minor.$((base_patch + 1))" ;;
    minor) next="$base_major.$((base_minor + 1)).0" ;;
    major) next="$((base_major + 1)).0.0" ;;
    *)     next="$bump" ;;
esac
[[ "$next" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "'$next' is not an X.Y.Z version"

# Never backwards, and never a version already released.
if [ -n "$released" ] && ! version_lt "$released" "$next"; then
    die "v$next is not newer than the last release, v$released"
fi
# `Cargo.toml` already ahead of the bump asked for: releasing would LOWER it.
if version_lt "$next" "$current"; then
    die "Cargo.toml is already at $current, ahead of the $bump bump (v$next) from v${released:-$current}.
  Release it as is:   scripts/release.sh $current
  or go past it:      scripts/release.sh <X.Y.Z newer than $current>"
fi
tag="v$next"

# Tagging the version already in Cargo.toml is allowed exactly once: it is
# how the first release happens. Once that tag exists the tag check below
# refuses it, so this cannot re-tag a release.
first_tag=0
[ "$next" != "$current" ] || first_tag=1

if [ "$dry_run" -eq 1 ]; then printf '\n(dry run — nothing will be changed)\n'; fi

# ---------------------------------------------------------------- preflight

say "preflight (v$current -> $tag)"

branch="$(git rev-parse --abbrev-ref HEAD)"
[ "$branch" = "main" ] || die "on branch '$branch'; releases are cut from main"

# CHANGELOG.md may be dirty: writing the notes and cutting the release is one
# motion, and both land in the release commit.
dirty="$(git status --porcelain -- . ':!CHANGELOG.md')"
[ -z "$dirty" ] || die "working tree is not clean (only CHANGELOG.md may be modified):
$dirty"

git fetch --quiet --tags origin main || die "git fetch origin main failed"
[ "$(git rev-parse HEAD)" = "$(git rev-parse origin/main)" ] \
    || die "HEAD is not origin/main — pull or push first"

if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
    die "tag $tag already exists locally"
fi
[ -z "$(git ls-remote --tags origin "refs/tags/$tag")" ] \
    || die "tag $tag already exists on origin"
# Tags were just fetched: re-check against the newest release anyone has cut,
# not only the ones this clone knew about when the version was computed.
released_now="$(last_release)"
if [ -n "$released_now" ] && ! version_lt "$released_now" "$next"; then
    die "v$next is not newer than the last release on origin, v$released_now"
fi
echo "  on main, in sync with origin/main, $tag is free"

# The README names the release twice, and the bump below finds both by
# pattern. Checked HERE, before anything is touched, so a README edited out
# of shape stops the release with the tree as it was.
grep -qE '^\| Version \| `v[0-9]+\.[0-9]+\.[0-9]+`' README.md \
    || die "README.md has no '| Version | \`vX.Y.Z\`' row for the bump to rewrite"
grep -qE 'rev: Some\("v[0-9]+\.[0-9]+\.[0-9]+"\.to_string\(\)\)' README.md \
    || die "README.md has no 'rev: Some(\"vX.Y.Z\".to_string())' pin for the bump to rewrite"
grep -qE 'version: Some\("v[0-9]+\.[0-9]+\.[0-9]+"\.to_string\(\)\)' README.md \
    || die "README.md has no 'version: Some(\"vX.Y.Z\".to_string())' pin for the bump to rewrite"
echo "  builds against $api"

# ---------------------------------------------------------------- changelog

say "changelog"

if ! extract_section "$next" | grep -q . \
    && extract_section Unreleased | tail -n +2 | grep -q '[^[:space:]]'; then
    if [ "$dry_run" -eq 1 ]; then
        echo "  would retitle '## Unreleased' as '## $next — $(date +%F)'"
        echo "  would record: Builds against $api."
    else
        rewrite CHANGELOG.md -v ver="$next" -v day="$(date +%F)" -v api="$api" '
            !done && /^## Unreleased[ \t]*$/ {
                print "## " ver " — " day
                print ""
                print "Builds against " api "."
                done = 1
                next
            }
            { print }
        '
        echo "  '## Unreleased' retitled '## $next — $(date +%F)'"
    fi
fi

if [ "$dry_run" -eq 0 ] && ! extract_section "$next" | grep -q .; then
    rewrite CHANGELOG.md -v ver="$next" -v day="$(date +%F)" -v api="$api" -v marker="$STUB_MARKER" '
        { print }
        !inserted && $0 == "# Changelog" {
            print ""
            print "## " ver " — " day
            print ""
            print "Builds against " api "."
            print ""
            print marker
            inserted = 1
        }
    '
    die "CHANGELOG.md had no '## $next' section — a dated stub is now in place.
Write the notes, then re-run:
    scripts/release.sh $bump"
fi

if [ "$dry_run" -eq 0 ]; then
    section="$(extract_section "$next")"
    case "$section" in
        *"$STUB_MARKER"*) die "the '## $next' section of CHANGELOG.md is still the stub — write it first" ;;
    esac
    echo "  '## $next' section present — it becomes the tag message"
elif ! extract_section "$next" | grep -q . \
    && ! extract_section Unreleased | tail -n +2 | grep -q '[^[:space:]]'; then
    die "CHANGELOG.md has neither '## $next' nor a written '## Unreleased' section"
fi

# --------------------------------------------------------------------- bump

say "bump"

if [ "$first_tag" -eq 1 ]; then
    echo "  first release: tagging v$current as it stands, no bump"
elif [ "$dry_run" -eq 1 ]; then
    echo "  would set the version to $next in Cargo.toml, Cargo.lock and README.md"
else
    rewrite Cargo.toml -v ver="$next" '
        !done && /^version[[:space:]]*=/ { print "version = \"" ver "\""; done = 1; next }
        { print }
    '
    [ "$(read_version)" = "$next" ] || die "Cargo.toml still reads '$(read_version)'"

    # This package's entry only: the line after its `name`.
    rewrite Cargo.lock -v ver="$next" '
        prev ~ /^name = "lattice-org-plugin"$/ && /^version = / { print "version = \"" ver "\""; prev = $0; next }
        { print; prev = $0 }
    '
    grep -A1 -x 'name = "lattice-org-plugin"' Cargo.lock | grep -qx "version = \"$next\"" \
        || die "Cargo.lock's lattice-org-plugin entry was not rewritten"
    echo "  Cargo.toml, Cargo.lock: $current -> $next"
fi

# The README is checked on every path, the first release included: the
# snippet a user copies must name the release being tagged.
if [ "$dry_run" -eq 0 ]; then
    rewrite README.md -v ver="$next" '
        /^\| Version \| `v[0-9]+\.[0-9]+\.[0-9]+`/ {
            sub(/`v[0-9]+\.[0-9]+\.[0-9]+`/, "`v" ver "`")
        }
        /(rev|version): Some\("v[0-9]+\.[0-9]+\.[0-9]+"\.to_string\(\)\)/ {
            sub(/"v[0-9]+\.[0-9]+\.[0-9]+"/, "\"v" ver "\"")
        }
        { print }
    '
fi
if [ "$dry_run" -eq 1 ]; then
    echo "  would set README.md's '| Version |' row and 'rev:' / 'version:' pins to $tag"
else
    grep -qE "^\| Version \| \`v$next\`" README.md \
        && grep -qF "rev: Some(\"v$next\".to_string())" README.md \
        && grep -qF "version: Some(\"v$next\".to_string())" README.md \
        || die "README.md was not rewritten to $tag — 'git diff README.md'"
    echo "  README.md names $tag"
fi

# ---------------------------------------------------------- commit and tag

say "commit and tag"

if [ "$dry_run" -eq 1 ]; then
    echo "  would git add Cargo.toml Cargo.lock README.md CHANGELOG.md"
    echo "  would git commit -m 'chore(release): $tag'"
    echo "  would git tag -a $tag  (message = the '## $next' changelog section)"
    printf '\n(dry run — nothing was changed)\n\n'
    exit 0
fi

# Explicit paths, never `git add -A`.
git add Cargo.toml Cargo.lock README.md CHANGELOG.md
git commit --quiet -m "chore(release): $tag"
# --cleanup=whitespace: the default `strip` deletes every `#`-leading line,
# which is the section's heading and every subheading in it.
printf '%s\n' "$section" | git tag -a "$tag" --cleanup=whitespace -F -
echo "  committed, and tagged $tag"

cat <<NEXT

=== next

  Nothing has been pushed. Review:

      git show --stat HEAD
      git show $tag

  Publish it:

      git push origin main && git push origin $tag

  Or undo:

      git tag -d $tag && git reset --hard HEAD~1

NEXT
