#!/usr/bin/env bash
#
# Everything that has to be true before markdoc-prose is published.
#
# ONE COPY, as in tw and markdoc-lint: ci.yml runs it on every push and pull
# request, release.yml runs it before a tag publishes anything. A second list
# of steps drifts, and the half that drifts is always the one nobody
# watches.
#
# Inputs:
#   $1          the markdoc-prose checkout                default: $PWD
#   STRICT      1 to fail on clippy warnings              default: 1
set -euo pipefail

ROOT="${1:-$PWD}"
cd "$ROOT"
ROOT="$PWD"
STRICT="${STRICT:-1}"

group() { echo "::group::$*"; }
endgroup() { echo "::endgroup::"; }
die() { echo "::error::$*" >&2; exit 1; }

[ -f Cargo.toml ] || die "no Cargo.toml in $ROOT — point this at the markdoc-prose checkout"

failed=""
step() {
  local name="$1"; shift
  group "$name"
  if "$@"; then
    endgroup
  else
    endgroup
    echo "::error::$name failed"
    failed="$failed $name"
  fi
}

# Formatting first: it is the cheapest, and a reformat in the middle of a
# clippy fix is a confusing diff to review.
step "cargo fmt" cargo fmt --check

if [ "$STRICT" = "1" ]; then
  step "cargo clippy" cargo clippy --all-targets -- -D warnings
else
  step "cargo clippy" cargo clippy --all-targets
fi

# The library tests and the binary's. There are no doctests to lose to
# `--all-targets`: no doc comment here holds a code block.
step "cargo test" cargo test --all-targets

# The release profile is what ships, and it differs from debug in more than
# speed: an overflow that panics in debug wraps in release.
step "cargo build --release" cargo build --release

# A binary that will not start is worse than one that was never built: the
# consumer caches it and reports a tool that is "installed". This catches a
# missing dynamic dependency before a release does.
step "it runs" ./target/release/markdoc-prose --version

# And the engine is really in it. `--rules` loads a config, builds harper's
# rule group and lists it, which is everything but the linting. Run against
# an empty directory, so the defaults are what is listed whatever this
# checkout holds; with no config the spell checker is on, so a list without
# it is an engine that did not load. Captured and then searched, rather than
# piped into `grep -q`: grep exiting at the first match leaves the binary
# writing to a closed pipe, and under pipefail that fails a passing check.
lists_its_rules() {
  local empty out
  empty="$(mktemp -d)" || return 1
  if ! out="$(./target/release/markdoc-prose --rules --root "$empty")"; then
    rmdir "$empty"
    return 1
  fi
  rmdir "$empty"
  echo "$(grep -c . <<<"$out" || true) rules listed"
  grep -qx 'on  SpellCheck' <<<"$out" || {
    echo "no 'on  SpellCheck' in the default rule list" >&2
    return 1
  }
}
step "it lists its rules" lists_its_rules

for s in .github/scripts/*.sh; do
  step "bash -n $(basename "$s")" bash -n "$s"
done

if [ -n "$failed" ]; then
  die "failed:$failed"
fi
echo "==> all checks passed"
