#!/usr/bin/env bash
# Refuse unless ci.yml went green on a push to main for this exact commit.
#
# Usage: scripts/ci_green.sh <sha>     (needs GH_TOKEN, GH_REPO; actions: read)
#
# release.yml, publish-cli.yml and publish-loader.yml all ship a main commit.
# CI already ran every suite on it (Linux + Windows pytest, Lua spec, TS,
# frozen sidecars, loader cross-compile), so they wait for that run instead of
# re-running a subset of it. A commit that never went through CI on main has no
# run and is refused. Only the external link check is exempt: a third-party
# site being down says nothing about the build. v5.9.10 shipped from a commit
# whose CI was red because nothing looked at it; this is that look.
set -euo pipefail
SHA="${1:?usage: ci_green.sh <sha>}"

id=""
for _ in $(seq 1 20); do
  id=$(gh run list --workflow ci.yml --commit "$SHA" --event push \
         --limit 1 --json databaseId --jq '.[0].databaseId // empty')
  [ -n "$id" ] && break
  echo "no CI run for $SHA yet; waiting"; sleep 30
done
if [ -z "$id" ]; then
  echo "::error::no CI run for $SHA — push the commit to main and let CI finish first"
  exit 1
fi

gh run watch "$id" --interval 30 > /dev/null
bad=$(gh run view "$id" --json jobs --jq \
  '.jobs[] | select(.name | startswith("docs external links") | not)
           | select(.conclusion != "success" and .conclusion != "skipped")
           | "\(.name): \(.conclusion)"')
if [ -n "$bad" ]; then
  echo "::error::CI is not green on $SHA (run $id):"
  echo "$bad"
  exit 1
fi
echo "CI run $id is green on $SHA"
