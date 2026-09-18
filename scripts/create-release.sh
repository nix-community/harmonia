#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" >/dev/null && pwd )"
cd "$SCRIPT_DIR/.."

repo=nix-community/harmonia

version=${1:-}
if [[ -z "$version" ]]; then
  echo "USAGE: $0 version" >&2
  exit 1
fi

# Validate semver (MAJOR.MINOR.PATCH with optional pre-release and build metadata)
semver_re='^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(-([0-9A-Za-z-]+\.)*[0-9A-Za-z-]+)?(\+([0-9A-Za-z-]+\.)*[0-9A-Za-z-]+)?$'
if [[ ! "$version" =~ $semver_re ]]; then
  echo "error: '$version' is not a valid semver version" >&2
  exit 1
fi
tag="harmonia-v${version}"
branch="release-${version}"

if [[ "$(git symbolic-ref --short HEAD)" != "main" ]]; then
  echo "must be on main branch" >&2
  exit 1
fi

# ensure we are up-to-date
uncommited_changes=$(git diff --compact-summary)
if [[ -n "$uncommited_changes" ]]; then
  echo -e "There are uncommited changes, exiting:\n${uncommited_changes}" >&2
  exit 1
fi
git pull "git@github.com:${repo}" main
unpushed_commits=$(git log --format=oneline origin/main..main)
if [[ "$unpushed_commits" != "" ]]; then
  echo -e "\nThere are unpushed changes, exiting:\n$unpushed_commits" >&2
  exit 1
fi
if git ls-remote --exit-code --tags origin "$tag" >/dev/null; then
  echo "Tag ${tag} already exists on origin, exiting" >&2
  exit 1
fi

# Get current version before bumping
old_version=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml)

# Update workspace version in root Cargo.toml
sed -i -e "s!^version = \".*\"\$!version = \"${version}\"!" Cargo.toml

# Update inter-crate dependency version specifiers in all sub-crate Cargo.toml files
for toml in */Cargo.toml; do
  sed -i -e "s!\(harmonia-[a-z-]* = {.*version = \)\"${old_version}\"!\1\"${version}\"!g" "$toml"
done

cargo update --workspace
git add Cargo.lock Cargo.toml ./**/Cargo.toml

# main is protected, so the bump goes through a PR and the tag is created on
# whatever commit actually lands on main.
git branch -D "$branch" 2>/dev/null || true
git checkout -b "$branch"
git commit -m "bump version ${tag}"
git push --force origin "$branch"
pr_url=$(gh pr create \
  --repo "$repo" \
  --base main \
  --head "$branch" \
  --title "bump version ${tag}" \
  --body "Release ${version}")
gh pr merge --repo "$repo" "$pr_url" --auto
git checkout main

while [[ "$(gh pr view --repo "$repo" "$pr_url" --json state --jq .state)" != "MERGED" ]]; do
  echo "Waiting for ${pr_url} to be merged..."
  sleep 30
done

git pull "git@github.com:${repo}" main
git tag "$tag"
git push origin "$tag"
gh release create --repo "$repo" "$tag" --title "$tag" --generate-notes
