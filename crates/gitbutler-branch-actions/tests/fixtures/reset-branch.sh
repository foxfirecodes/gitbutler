#!/usr/bin/env bash
set -eu -o pipefail

function commit_exact () {
  local message=${1:?}
  git add -A
  local tree
  tree=$(git write-tree)
  local parent_args=()
  if git rev-parse --verify HEAD >/dev/null 2>&1; then
    parent_args=(-p HEAD)
  fi
  local commit
  commit=$(printf "%s" "$message" | git commit-tree "$tree" "${parent_args[@]}")
  local current_branch
  current_branch=$(git symbolic-ref -q HEAD || true)
  if [[ -n "$current_branch" ]]; then
    git update-ref "$current_branch" "$commit"
  fi
  git reset --hard "$commit" >/dev/null
}

git init --initial-branch=main remote
(cd remote
  git config user.name "Author"
  git config user.email "author@example.com"
  echo init > file
  git add . && git commit -m "init"
)

# Scenario: single-branch
# A single branch with 2 commits, all pushed to remote.
#
# - commit 2 (my_stack) [pushed]
# - commit 1            [pushed]
git clone remote single-branch
(cd single-branch
  git config user.name "Author"
  git config user.email "author@example.com"

  git checkout --detach main
  echo change1 > file1
  commit_exact "commit 1"
  commit_1=$(git rev-parse HEAD)

  echo change2 > file2
  commit_exact "commit 2"
  commit_2=$(git rev-parse HEAD)

  git branch my_stack "$commit_2"

  # Simulate push: copy local ref to remote tracking ref
  mkdir -p .git/refs/remotes/origin
  cp .git/refs/heads/my_stack .git/refs/remotes/origin/my_stack

  git checkout -b gitbutler/workspace "$commit_2"
  git commit --allow-empty -m "GitButler Workspace Commit"
)

# Scenario: single-branch-diverged
# A single branch with 2 commits pushed, then locally squashed into 1.
# Local and remote have diverged.
#
# Local:
# - squashed (my_stack)
#
# Remote:
# - commit 2 (origin/my_stack)
# - commit 1
git clone remote single-branch-diverged
(cd single-branch-diverged
  git config user.name "Author"
  git config user.email "author@example.com"

  git checkout --detach main
  echo change1 > file1
  commit_exact "commit 1"
  commit_1=$(git rev-parse HEAD)

  echo change2 > file2
  commit_exact "commit 2"
  commit_2=$(git rev-parse HEAD)

  # Set up remote tracking with the original 2 commits
  mkdir -p .git/refs/remotes/origin
  git update-ref refs/remotes/origin/my_stack "$commit_2"

  # Now squash locally: create a single commit with both changes on top of main
  git checkout --detach main
  echo change1 > file1
  echo change2 > file2
  commit_exact "squashed"
  squashed=$(git rev-parse HEAD)

  git branch my_stack "$squashed"

  git checkout -b gitbutler/workspace "$squashed"
  git commit --allow-empty -m "GitButler Workspace Commit"
)

# Scenario: stacked-branches
# Two stacked branches, both pushed to remote.
#
# - commit 3 (a-branch-2) [pushed]
# - commit 2 (my_stack)   [pushed]
# - commit 1              [pushed]
git clone remote stacked-branches
(cd stacked-branches
  git config user.name "Author"
  git config user.email "author@example.com"

  git checkout --detach main
  echo change1 > file1
  commit_exact "commit 1"
  commit_1=$(git rev-parse HEAD)

  echo change2 > file2
  commit_exact "commit 2"
  commit_2=$(git rev-parse HEAD)

  echo change3 > file3
  commit_exact "commit 3"
  commit_3=$(git rev-parse HEAD)

  git branch my_stack "$commit_2"
  git branch a-branch-2 "$commit_3"

  # Simulate push for both branches
  mkdir -p .git/refs/remotes/origin
  cp .git/refs/heads/my_stack .git/refs/remotes/origin/my_stack
  cp .git/refs/heads/a-branch-2 .git/refs/remotes/origin/a-branch-2

  git checkout -b gitbutler/workspace "$commit_3"
  git commit --allow-empty -m "GitButler Workspace Commit"
)

# Scenario: stacked-branches-base-diverged
# Two stacked branches. Base is squashed locally, top is unchanged.
# This simulates: user squashed base branch, now wants to reset to remote.
#
# Local:
# - commit 3 (a-branch-2, rebased onto squashed)
# - squashed (my_stack)
#
# Remote:
# - commit 3 (origin/a-branch-2)
# - commit 2 (origin/my_stack)
# - commit 1
git clone remote stacked-branches-base-diverged
(cd stacked-branches-base-diverged
  git config user.name "Author"
  git config user.email "author@example.com"

  git checkout --detach main
  echo change1 > file1
  commit_exact "commit 1"
  commit_1=$(git rev-parse HEAD)

  echo change2 > file2
  commit_exact "commit 2"
  commit_2=$(git rev-parse HEAD)

  echo change3 > file3
  commit_exact "commit 3"
  commit_3=$(git rev-parse HEAD)

  # Set up remote tracking with the original commits
  mkdir -p .git/refs/remotes/origin
  git update-ref refs/remotes/origin/my_stack "$commit_2"
  git update-ref refs/remotes/origin/a-branch-2 "$commit_3"

  # Squash base branch locally
  git checkout --detach main
  echo change1 > file1
  echo change2 > file2
  commit_exact "squashed"
  squashed=$(git rev-parse HEAD)

  # Rebase top branch onto squashed base
  echo change3 > file3
  commit_exact "commit 3 rebased"
  commit_3_rebased=$(git rev-parse HEAD)

  git branch my_stack "$squashed"
  git branch a-branch-2 "$commit_3_rebased"

  git checkout -b gitbutler/workspace "$commit_3_rebased"
  git commit --allow-empty -m "GitButler Workspace Commit"
)

# Scenario: stacked-three-branches
# Three stacked branches, all pushed.
#
# - commit 5 (a-branch-3) [pushed]
# - commit 4 (a-branch-2) [pushed]
# - commit 3              [pushed]
# - commit 2 (my_stack)   [pushed]
# - commit 1              [pushed]
git clone remote stacked-three-branches
(cd stacked-three-branches
  git config user.name "Author"
  git config user.email "author@example.com"

  git checkout --detach main
  echo change1 > file1
  commit_exact "commit 1"
  commit_1=$(git rev-parse HEAD)

  echo change2 > file2
  commit_exact "commit 2"
  commit_2=$(git rev-parse HEAD)

  echo change3 > file3
  commit_exact "commit 3"
  commit_3=$(git rev-parse HEAD)

  echo change4 > file4
  commit_exact "commit 4"
  commit_4=$(git rev-parse HEAD)

  echo change5 > file5
  commit_exact "commit 5"
  commit_5=$(git rev-parse HEAD)

  git branch my_stack "$commit_2"
  git branch a-branch-2 "$commit_4"
  git branch a-branch-3 "$commit_5"

  mkdir -p .git/refs/remotes/origin
  cp .git/refs/heads/my_stack .git/refs/remotes/origin/my_stack
  cp .git/refs/heads/a-branch-2 .git/refs/remotes/origin/a-branch-2
  cp .git/refs/heads/a-branch-3 .git/refs/remotes/origin/a-branch-3

  git checkout -b gitbutler/workspace "$commit_5"
  git commit --allow-empty -m "GitButler Workspace Commit"
)
