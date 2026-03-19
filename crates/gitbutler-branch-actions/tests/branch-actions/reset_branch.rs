use anyhow::Result;
use but_ctx::Context;
use but_workspace::ui::BranchDetails;
use gitbutler_branch_actions::{reset_branch_to_remote, squash_commits};
use gitbutler_stack::VirtualBranchesHandle;
use tempfile::TempDir;

use crate::driverless;

// Reset a single branch (no stack) to its remote state.
// The local and remote are identical, so this should be a no-op.
//
// Before:
// - commit 2 (my_stack) [pushed]
// - commit 1            [pushed]
//
// After: same
#[test]
fn reset_single_branch_noop() -> Result<()> {
    let (mut ctx, _temp_dir) = ctx("single-branch")?;
    let stack = find_stack(&ctx)?;

    reset_branch_to_remote(&mut ctx, stack.id, "my_stack".into())?;

    let branches = list_branches(&ctx);
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].commits.len(), 2);
    assert_eq!(branches[0].commits[0].message, "commit 2");
    assert_eq!(branches[0].commits[1].message, "commit 1");
    Ok(())
}

// Reset a single branch after local squash (diverged from remote).
// This was the "Picked commit cannot be the base commit" bug.
//
// Before (local):
// - squashed (my_stack)
//
// Before (remote):
// - commit 2 (origin/my_stack)
// - commit 1
//
// After:
// - commit 2 (my_stack)
// - commit 1
#[test]
fn reset_single_branch_after_squash() -> Result<()> {
    let (mut ctx, _temp_dir) = ctx("single-branch-diverged")?;
    let stack = find_stack(&ctx)?;

    reset_branch_to_remote(&mut ctx, stack.id, "my_stack".into())?;

    let branches = list_branches(&ctx);
    assert_eq!(branches.len(), 1);
    assert_eq!(branches[0].commits.len(), 2);
    assert_eq!(branches[0].commits[0].message, "commit 2");
    assert_eq!(branches[0].commits[1].message, "commit 1");
    Ok(())
}

// Reset the bottom branch of a two-branch stack.
// The top branch should be rebased on top.
//
// Before:
// - commit 3 (a-branch-2) [pushed]
// - commit 2 (my_stack)   [pushed]
// - commit 1              [pushed]
//
// After: same structure (commits preserved, top branch rebased)
#[test]
fn reset_base_branch_in_stack() -> Result<()> {
    let (mut ctx, _temp_dir) = ctx("stacked-branches")?;
    let stack = find_stack(&ctx)?;

    reset_branch_to_remote(&mut ctx, stack.id, "my_stack".into())?;

    let branches = list_branches(&ctx);
    assert_eq!(branches.len(), 2);

    let base = branches.iter().find(|b| b.name == "my_stack").unwrap();
    assert_eq!(base.commits.len(), 2);
    assert_eq!(base.commits[0].message, "commit 2");
    assert_eq!(base.commits[1].message, "commit 1");

    let top = branches.iter().find(|b| b.name == "a-branch-2").unwrap();
    assert_eq!(top.commits.len(), 1);
    assert_eq!(top.commits[0].message, "commit 3");
    Ok(())
}

// Reset the top branch of a two-branch stack.
// The bottom branch should be unaffected.
//
// Before:
// - commit 3 (a-branch-2) [pushed]
// - commit 2 (my_stack)   [pushed]
// - commit 1              [pushed]
//
// After: same
#[test]
fn reset_top_branch_in_stack() -> Result<()> {
    let (mut ctx, _temp_dir) = ctx("stacked-branches")?;
    let stack = find_stack(&ctx)?;

    reset_branch_to_remote(&mut ctx, stack.id, "a-branch-2".into())?;

    let branches = list_branches(&ctx);
    assert_eq!(branches.len(), 2);

    let base = branches.iter().find(|b| b.name == "my_stack").unwrap();
    assert_eq!(base.commits.len(), 2);

    let top = branches.iter().find(|b| b.name == "a-branch-2").unwrap();
    assert_eq!(top.commits.len(), 1);
    assert_eq!(top.commits[0].message, "commit 3");
    Ok(())
}

// Reset the base branch of a stack after it was locally squashed.
// This was the "Picked commit cannot be the base commit" bug.
//
// Before (local):
// - commit 3 rebased (a-branch-2)
// - squashed (my_stack)
//
// Before (remote):
// - commit 3 (origin/a-branch-2)
// - commit 2 (origin/my_stack)
// - commit 1
//
// After resetting my_stack:
// - commit 3 rebased (a-branch-2) [rebased onto restored base]
// - commit 2 (my_stack)
// - commit 1
#[test]
fn reset_base_branch_after_squash_in_stack() -> Result<()> {
    let (mut ctx, _temp_dir) = ctx("stacked-branches-base-diverged")?;
    let stack = find_stack(&ctx)?;

    reset_branch_to_remote(&mut ctx, stack.id, "my_stack".into())?;

    let branches = list_branches(&ctx);
    assert_eq!(branches.len(), 2);

    let base = branches.iter().find(|b| b.name == "my_stack").unwrap();
    assert_eq!(base.commits.len(), 2);
    assert_eq!(base.commits[0].message, "commit 2");
    assert_eq!(base.commits[1].message, "commit 1");

    // Top branch should still have one commit (rebased)
    let top = branches.iter().find(|b| b.name == "a-branch-2").unwrap();
    assert_eq!(top.commits.len(), 1);
    Ok(())
}

// Reset the top branch of a stack after the base was squashed.
// This was the "Picked commit already exists in a previous step" bug
// caused by collecting too many remote commits (including base branch commits).
//
// Before (local):
// - commit 3 rebased (a-branch-2)
// - squashed (my_stack)
//
// After resetting a-branch-2:
// The top branch should be reset to its remote commits only.
#[test]
fn reset_top_branch_after_base_squash_in_stack() -> Result<()> {
    let (mut ctx, _temp_dir) = ctx("stacked-branches-base-diverged")?;
    let stack = find_stack(&ctx)?;

    // First reset the base
    reset_branch_to_remote(&mut ctx, stack.id, "my_stack".into())?;

    // Then reset the top branch — this previously failed with duplicate commit error
    reset_branch_to_remote(&mut ctx, stack.id, "a-branch-2".into())?;

    let branches = list_branches(&ctx);
    assert_eq!(branches.len(), 2);

    let base = branches.iter().find(|b| b.name == "my_stack").unwrap();
    assert_eq!(base.commits.len(), 2);
    assert_eq!(base.commits[0].message, "commit 2");
    assert_eq!(base.commits[1].message, "commit 1");

    let top = branches.iter().find(|b| b.name == "a-branch-2").unwrap();
    assert_eq!(top.commits.len(), 1);
    assert_eq!(top.commits[0].message, "commit 3");
    Ok(())
}

// Reset the middle branch of a three-branch stack.
// Branches below should be unaffected, branches above should be rebased.
//
// Before:
// - commit 5 (a-branch-3)
// - commit 4 (a-branch-2)
// - commit 3
// - commit 2 (my_stack)
// - commit 1
//
// After: same structure
#[test]
fn reset_middle_branch_in_three_branch_stack() -> Result<()> {
    let (mut ctx, _temp_dir) = ctx("stacked-three-branches")?;
    let stack = find_stack(&ctx)?;

    reset_branch_to_remote(&mut ctx, stack.id, "a-branch-2".into())?;

    let branches = list_branches(&ctx);
    assert_eq!(branches.len(), 3);

    let base = branches.iter().find(|b| b.name == "my_stack").unwrap();
    assert_eq!(base.commits.len(), 2);
    assert_eq!(base.commits[0].message, "commit 2");
    assert_eq!(base.commits[1].message, "commit 1");

    let middle = branches.iter().find(|b| b.name == "a-branch-2").unwrap();
    assert_eq!(middle.commits.len(), 2);
    assert_eq!(middle.commits[0].message, "commit 4");
    assert_eq!(middle.commits[1].message, "commit 3");

    let top = branches.iter().find(|b| b.name == "a-branch-3").unwrap();
    assert_eq!(top.commits.len(), 1);
    assert_eq!(top.commits[0].message, "commit 5");
    Ok(())
}

// Reset a branch that has no remote tracking branch should fail gracefully.
#[test]
fn reset_branch_without_remote_fails() -> Result<()> {
    let (mut ctx, _temp_dir) = ctx("single-branch")?;
    let stack = find_stack(&ctx)?;

    // Remove the remote tracking ref to simulate no remote
    let repo = ctx.repo.get()?;
    let remote_ref = repo.find_reference("refs/remotes/origin/my_stack")?;
    remote_ref.delete()?;
    drop(repo);

    let result = reset_branch_to_remote(&mut ctx, stack.id, "my_stack".into());
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("no remote tracking branch")
    );
    Ok(())
}

// After squashing the base branch in a stacked setup, then resetting it,
// verify file content is restored correctly.
#[test]
fn reset_preserves_file_content() -> Result<()> {
    let (mut ctx, _temp_dir) = ctx("stacked-branches")?;
    let stack = find_stack(&ctx)?;

    // Squash the base branch's two commits into one
    let repo = ctx.repo.get()?;
    let base_branch = stack
        .branches()
        .iter()
        .find(|b| b.name() == "my_stack")
        .unwrap()
        .clone();
    let commits = base_branch.commit_ids(&repo, &ctx, &stack)?.local_commits;
    drop(repo);

    // commits are in oldest-to-newest order: [commit_1, commit_2]
    assert_eq!(commits.len(), 2);
    squash_commits(&mut ctx, stack.id, vec![commits[1]], commits[0])?;

    // Verify squash happened (1 commit now)
    let branches_before = list_branches(&ctx);
    let base_before = branches_before
        .iter()
        .find(|b| b.name == "my_stack")
        .unwrap();
    assert_eq!(base_before.commits.len(), 1);

    // Reset to remote
    reset_branch_to_remote(&mut ctx, stack.id, "my_stack".into())?;

    // Verify we're back to 2 commits
    let branches_after = list_branches(&ctx);
    let base_after = branches_after
        .iter()
        .find(|b| b.name == "my_stack")
        .unwrap();
    assert_eq!(base_after.commits.len(), 2);
    assert_eq!(base_after.commits[0].message, "commit 2");
    assert_eq!(base_after.commits[1].message, "commit 1");
    Ok(())
}

fn ctx(scenario: &str) -> Result<(Context, TempDir)> {
    driverless::writable_context("reset-branch.sh", scenario)
}

fn find_stack(ctx: &Context) -> Result<gitbutler_stack::Stack> {
    let handle = VirtualBranchesHandle::new(ctx.project_data_dir());
    let stacks = handle.list_all_stacks()?;
    let names: Vec<_> = stacks.iter().map(|s| s.name().to_string()).collect();
    Ok(stacks
        .into_iter()
        .find(|s| s.in_workspace)
        .unwrap_or_else(|| panic!("No in-workspace stack found. Stack names: {names:?}")))
}

fn list_branches(ctx: &Context) -> Vec<BranchDetails> {
    let details = gitbutler_testsupport::stack_details(ctx);
    let (_, details) = details.first().unwrap();
    details.branch_details.clone()
}
