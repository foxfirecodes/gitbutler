use anyhow::{Context as _, Result, bail};
use but_ctx::{Context, access::RepoExclusive};
use but_oxidize::{ObjectIdExt, OidExt};
use but_rebase::{Rebase, RebaseStep};
use but_workspace::legacy::stack_ext::StackExt;
use gitbutler_stack::{StackId, VirtualBranchesHandle};
use gitbutler_workspace::branch_trees::{WorkspaceState, update_uncommitted_changes};

/// Reset a branch in a stack to match its remote tracking branch.
///
/// This replaces all local commits in the target branch with the commits
/// from the remote tracking branch, then rebases any branches above it
/// in the stack on top.
pub fn reset_branch_to_remote(
    ctx: &Context,
    stack_id: StackId,
    branch_name: String,
    perm: &mut RepoExclusive,
) -> Result<()> {
    let old_workspace = WorkspaceState::create(ctx, perm.read_permission())?;
    let repo = ctx.repo.get()?;
    let vb_state = VirtualBranchesHandle::new(ctx.project_data_dir());

    let mut source_stack = vb_state.get_stack_in_workspace(stack_id)?;
    let merge_base = source_stack.merge_base(ctx)?;

    // Find the branch reference
    let branch_ref = repo
        .try_find_reference(&branch_name)?
        .ok_or_else(|| anyhow::anyhow!("Branch '{branch_name}' not found in repository"))?;
    let branch_ref_name = branch_ref.name().to_owned();

    // Construct the remote tracking ref using the push remote name,
    // matching how GitButler resolves remote refs for virtual branches
    let push_remote = vb_state
        .get_default_target()
        .context("failed to get default target")?
        .push_remote_name();
    let remote_ref_name = format!("refs/remotes/{push_remote}/{branch_name}");

    let mut remote_ref = repo
        .find_reference(remote_ref_name.as_str())
        .map_err(|_| anyhow::anyhow!("Branch '{branch_name}' has no remote tracking branch"))?;
    let remote_tip = remote_ref.peel_to_id()?.detach();

    // Find where to stop collecting remote commits.
    // For stacked branches, we must only collect the commits belonging to THIS branch's
    // segment, not the entire ancestry (which would include the base branch's commits).
    // We do this by finding the remote tip of the branch below (if any).
    let remote_base = find_remote_base_for_branch(
        ctx,
        &repo,
        &source_stack,
        &branch_name,
        &push_remote,
        remote_tip,
        merge_base,
    )?;

    // Collect remote commits between remote_tip and remote_base (in child-first order for rebase)
    let remote_commits = collect_commits_between(&repo, remote_tip, remote_base)?;

    if remote_commits.is_empty() && remote_tip == remote_base {
        bail!("Remote branch has no commits to reset to");
    }

    // Build new rebase steps: walk the original steps, replacing the target branch's
    // picks with picks from the remote commits
    let original_rebase_steps = source_stack.as_rebase_steps_rev(ctx)?;
    let mut new_rebase_steps = vec![];

    let mut inside_branch = false;

    for step in original_rebase_steps {
        if let RebaseStep::Reference(but_core::Reference::Git(name)) = &step {
            if *name == branch_ref_name {
                inside_branch = true;
            } else if inside_branch {
                inside_branch = false;
            }
        }

        if let RebaseStep::Reference(but_core::Reference::Virtual(name)) = &step {
            if *name == branch_name {
                inside_branch = true;
            } else if inside_branch {
                inside_branch = false;
            }
        }

        if !inside_branch {
            new_rebase_steps.push(step);
            continue;
        }

        match &step {
            RebaseStep::Pick { .. } | RebaseStep::SquashIntoPreceding { .. } => {
                // Skip all existing picks for this branch - we'll replace them
                continue;
            }
            RebaseStep::Reference(_) => {
                // Keep the reference marker, then insert remote commits
                new_rebase_steps.push(step);
                for commit_id in &remote_commits {
                    new_rebase_steps.push(RebaseStep::Pick {
                        commit_id: *commit_id,
                        new_message: None,
                    });
                }
                continue;
            }
        }
    }

    new_rebase_steps.reverse();

    let mut rebase = Rebase::new(&repo, merge_base, None)?;
    rebase.steps(new_rebase_steps)?;
    rebase.rebase_noops(false);
    let result = rebase.rebase(&*ctx.cache.get_cache()?)?;
    let head = result.top_commit.to_git2();

    source_stack.set_stack_head(&vb_state, &repo, head.to_gix())?;
    let new_workspace = WorkspaceState::create(ctx, perm.read_permission())?;
    update_uncommitted_changes(ctx, old_workspace, new_workspace, perm)?;
    source_stack.set_heads_from_rebase_output(ctx, result.references)?;

    crate::integration::update_workspace_commit_with_vb_state(&vb_state, ctx, false)?;

    Ok(())
}

/// Find the correct stopping point for collecting remote commits for a branch.
///
/// For the bottom branch in a stack, this is the merge base between the remote tip
/// and the stack's merge base (handles diverged histories like after a squash).
///
/// For stacked branches, this is the remote tip of the branch below. This ensures
/// we only collect commits for this branch's segment, not the entire ancestry chain
/// which would duplicate commits from lower branches.
fn find_remote_base_for_branch(
    ctx: &Context,
    repo: &gix::Repository,
    stack: &gitbutler_stack::Stack,
    branch_name: &str,
    push_remote: &str,
    remote_tip: gix::ObjectId,
    stack_merge_base: gix::ObjectId,
) -> Result<gix::ObjectId> {
    let branches: Vec<_> = but_workspace::legacy::stack_branches(stack.id, ctx)?;

    // stack_branches returns branches in top-to-bottom order (tip-most first),
    // so the branch "below" in the stack is at a higher index.
    let branch_idx = branches
        .iter()
        .position(|b| b.name == branch_name)
        .ok_or_else(|| anyhow::anyhow!("Branch '{branch_name}' not found in stack"))?;

    let is_bottom_branch = branch_idx == branches.len() - 1;
    if is_bottom_branch {
        // Bottom branch: use merge base between remote tip and target
        return Ok(repo
            .merge_base(remote_tip, stack_merge_base)
            .map(|id| id.detach())
            .unwrap_or(stack_merge_base));
    }

    // Stacked branch: try to use the remote tip of the branch below as the stopping point.
    // In top-to-bottom order, the branch below is at branch_idx + 1.
    let below_branch = &branches[branch_idx + 1];
    let below_remote_ref = format!("refs/remotes/{push_remote}/{}", below_branch.name);
    if let Ok(mut below_remote) = repo.find_reference(below_remote_ref.as_str()) {
        if let Ok(below_remote_tip) = below_remote.peel_to_id() {
            return Ok(below_remote_tip.detach());
        }
    }

    // Fallback: the branch below has no remote tracking branch.
    // Use the local tip of the branch below instead.
    let mut below_ref = repo
        .try_find_reference(&below_branch.name)?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Branch '{}' not found in repository",
                below_branch.name
            )
        })?;
    let below_tip = below_ref.peel_to_id()?.detach();

    // Compute merge base between our remote tip and the branch below's tip
    Ok(repo
        .merge_base(remote_tip, below_tip)
        .map(|id| id.detach())
        .unwrap_or(stack_merge_base))
}

/// Collect commits between `tip` and `base` (exclusive), returning them in
/// child-first order (same order as `as_rebase_steps_rev` uses).
fn collect_commits_between(
    repo: &gix::Repository,
    tip: gix::ObjectId,
    base: gix::ObjectId,
) -> Result<Vec<gix::ObjectId>> {
    let mut commits = vec![];
    let mut current = tip;
    while current != base {
        commits.push(current);
        let commit = repo.find_commit(current)?;
        let parents: Vec<_> = commit.parent_ids().collect();
        if parents.is_empty() {
            break;
        }
        current = parents[0].detach();
    }
    Ok(commits)
}
