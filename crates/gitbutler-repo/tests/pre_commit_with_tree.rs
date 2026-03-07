//! Tests for `pre_commit_with_tree` — the function that runs a pre-commit hook
//! against a proposed commit tree and returns the post-hook index tree.
//!
//! The key correctness property being tested here is **partial staging**:
//! when a hook stages only a portion of a file's changes (e.g. via
//! `git add -p` or direct index manipulation), the returned `post_hook_tree`
//! must reflect exactly what was staged — **not** the full worktree file.
//! Prior to this fix the code re-applied whole-file `DiffSpec`s from the
//! worktree, silently discarding any partial staging the hook had performed.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use anyhow::Result;
use but_ctx::Context;
use gitbutler_repo::hooks::{HookResult, pre_commit_with_tree};
use gitbutler_testsupport::temp_dir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a bare-minimum git repository with one committed file.
///
/// Returns `(Context, repo_dir_tmp, initial_tree_oid)` where `initial_tree_oid`
/// is the OID of the tree that was committed.
fn setup_repo_with_commit(content: &str) -> Result<(Context, tempfile::TempDir, git2::Oid)> {
    let tmp = temp_dir();
    let path = tmp.path();

    let repo = git2::Repository::init(path)?;

    // Configure minimal git identity so commits don't fail.
    let mut cfg = repo.config()?;
    cfg.set_str("user.name", "Test")?;
    cfg.set_str("user.email", "test@test.com")?;
    cfg.set_str("commit.gpgsign", "false")?;
    drop(cfg);

    fs::write(path.join("file.txt"), content)?;

    let mut index = repo.index()?;
    index.add_path(std::path::Path::new("file.txt"))?;
    index.write()?;
    let tree_oid = index.write_tree()?;
    let tree = repo.find_tree(tree_oid)?;
    let sig = git2::Signature::now("Test", "test@test.com")?;
    repo.commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])?;

    let gix_repo = gix::open(path)?;
    let ctx = Context::from_repo(gix_repo)?;
    Ok((ctx, tmp, tree_oid))
}

/// Install an executable hook script at `{git_dir}/hooks/{hook_name}`.
fn install_hook(repo: &git2::Repository, hook_name: &str, script: &str) -> Result<()> {
    let hooks_dir = repo.path().join("hooks");
    fs::create_dir_all(&hooks_dir)?;
    let hook_path = hooks_dir.join(hook_name);
    fs::write(&hook_path, script)?;
    #[cfg(unix)]
    fs::set_permissions(&hook_path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// When no pre-commit hook is configured the function returns
/// `HookResult::NotConfigured` and the post-hook tree equals the tree we
/// passed in (no index changes).
#[test]
fn pre_commit_with_tree_no_hook() -> Result<()> {
    let (ctx, _tmp, tree_oid) = setup_repo_with_commit("line 1\nline 2\nline 3\n")?;

    let (result, post_hook_tree) = pre_commit_with_tree(&ctx, tree_oid)?;

    assert_eq!(result, HookResult::NotConfigured);
    assert_eq!(
        post_hook_tree, tree_oid,
        "index should be unchanged when no hook is configured"
    );
    Ok(())
}

/// When a hook runs successfully without touching the index the post-hook tree
/// must equal the tree we passed in.
#[test]
#[cfg(unix)]
fn pre_commit_with_tree_hook_no_index_changes() -> Result<()> {
    let (ctx, _tmp, tree_oid) = setup_repo_with_commit("line 1\nline 2\nline 3\n")?;

    let repo = git2::Repository::open(ctx.repo.get()?.workdir().unwrap())?;
    install_hook(&repo, "pre-commit", "#!/bin/sh\nexit 0\n")?;

    let (result, post_hook_tree) = pre_commit_with_tree(&ctx, tree_oid)?;

    assert_eq!(result, HookResult::Success);
    assert_eq!(
        post_hook_tree, tree_oid,
        "index should be unchanged when hook does not stage anything"
    );
    Ok(())
}

/// When a hook stages a new/modified file the returned `post_hook_tree` must
/// differ from the tree that was passed in and reflect the hook's staged content.
#[test]
#[cfg(unix)]
fn pre_commit_with_tree_hook_stages_whole_file() -> Result<()> {
    let (ctx, _tmp, tree_oid) = setup_repo_with_commit("original content\n")?;

    // The hook writes new content to the file and stages it.
    let repo = git2::Repository::open(ctx.repo.get()?.workdir().unwrap())?;
    install_hook(
        &repo,
        "pre-commit",
        "#!/bin/sh\necho 'hook-added content' > file.txt\ngit add file.txt\n",
    )?;

    let (result, post_hook_tree) = pre_commit_with_tree(&ctx, tree_oid)?;

    assert_eq!(result, HookResult::Success);
    assert_ne!(
        post_hook_tree, tree_oid,
        "post-hook tree should differ when hook stages a change"
    );

    // Verify the post-hook tree contains the hook's staged content.
    let git2_repo = ctx.git2_repo.get()?;
    let post_tree = git2_repo.find_tree(post_hook_tree)?;
    let blob_oid = post_tree.get_name("file.txt").unwrap().id();
    let blob = git2_repo.find_blob(blob_oid)?;
    assert_eq!(
        std::str::from_utf8(blob.content())?,
        "hook-added content\n",
        "post-hook tree must contain the content staged by the hook"
    );
    Ok(())
}

/// **Core partial-staging test.**
///
/// When a hook stages only *part* of a file's worktree changes (simulated here
/// by writing a specific intermediate state to the index via
/// `git update-index --cacheinfo`), the `post_hook_tree` must reflect exactly
/// what was staged — not the full worktree content.
///
/// This is the scenario that was broken before the fix: the old code would
/// create a whole-file `DiffSpec` (empty `hunk_headers`) which caused
/// `apply_worktree_changes` to read the entire worktree file, discarding any
/// partial staging.
#[test]
#[cfg(unix)]
fn pre_commit_with_tree_hook_partially_stages_file() -> Result<()> {
    // Commit a file with 3 lines.
    let (ctx, _tmp, tree_oid) = setup_repo_with_commit("line 1\nline 2\nline 3\n")?;

    // Simulate the worktree having TWO changes to the file:
    //   - "line 1" → "line 1 CHANGED"
    //   - "line 3" → "line 3 CHANGED"
    let workdir = ctx.repo.get()?.workdir().unwrap().to_owned();
    fs::write(
        workdir.join("file.txt"),
        "line 1 CHANGED\nline 2\nline 3 CHANGED\n",
    )?;

    // The hook stages only the *first* change (line 1) and restores the worktree.
    // We use `git update-index --cacheinfo` to write a specific blob directly
    // into the index — this is the programmatic equivalent of `git add -p`.
    let hook_script = r#"#!/bin/sh
set -e
# Stage only the first change: "line 1 CHANGED\nline 2\nline 3\n"
PARTIAL_BLOB=$(printf 'line 1 CHANGED\nline 2\nline 3\n' | git hash-object -w --stdin)
git update-index --cacheinfo 100644,"$PARTIAL_BLOB",file.txt
# Restore the worktree to its two-change state so the index differs from worktree.
printf 'line 1 CHANGED\nline 2\nline 3 CHANGED\n' > file.txt
"#;

    let repo = git2::Repository::open(&workdir)?;
    install_hook(&repo, "pre-commit", hook_script)?;

    let (result, post_hook_tree) = pre_commit_with_tree(&ctx, tree_oid)?;

    assert_eq!(result, HookResult::Success);
    assert_ne!(
        post_hook_tree, tree_oid,
        "post-hook tree should differ from the input tree (hook staged a change)"
    );

    // The post-hook tree must contain ONLY the first change — not both.
    let git2_repo = ctx.git2_repo.get()?;
    let post_tree = git2_repo.find_tree(post_hook_tree)?;
    let blob_oid = post_tree.get_name("file.txt").unwrap().id();
    let blob = git2_repo.find_blob(blob_oid)?;
    let committed_content = std::str::from_utf8(blob.content())?;

    assert_eq!(
        committed_content,
        "line 1 CHANGED\nline 2\nline 3\n",
        "post-hook tree must contain only the partially staged content, not the full worktree"
    );
    assert!(
        !committed_content.contains("line 3 CHANGED"),
        "the second change (line 3) must NOT be in the post-hook tree because the hook did not stage it"
    );
    Ok(())
}

/// Verify that `pre_commit_with_tree` restores the git index to its original
/// state after the hook runs, even when the hook staged new content.
///
/// This exercises the `scopeguard` that resets the index after every call so
/// that pre-existing staged changes are never polluted by hook side-effects.
#[test]
#[cfg(unix)]
fn pre_commit_with_tree_index_restored_after_hook() -> Result<()> {
    let (ctx, _tmp, tree_oid) = setup_repo_with_commit("original\n")?;

    // Stage a change in the index *before* calling pre_commit_with_tree.
    // We want to confirm this staged state is preserved after the call.
    let workdir = ctx.repo.get()?.workdir().unwrap().to_owned();
    fs::write(workdir.join("file.txt"), "user staged change\n")?;
    {
        let repo = git2::Repository::open(&workdir)?;
        let mut index = repo.index()?;
        index.add_path(std::path::Path::new("file.txt"))?;
        index.write()?;
    }

    // Capture the index tree *before* the hook runs.
    let pre_call_index_tree = {
        let repo = git2::Repository::open(&workdir)?;
        repo.index()?.write_tree()?
    };

    // Install a hook that stages completely different content.
    let repo = git2::Repository::open(&workdir)?;
    install_hook(
        &repo,
        "pre-commit",
        "#!/bin/sh\necho 'hook content' > file.txt\ngit add file.txt\n",
    )?;

    let (result, _post_hook_tree) = pre_commit_with_tree(&ctx, tree_oid)?;
    assert_eq!(result, HookResult::Success);

    // After the call the index must be back to the user's pre-call staged state.
    let post_call_index_tree = {
        let repo = git2::Repository::open(&workdir)?;
        repo.index()?.write_tree()?
    };

    assert_eq!(
        post_call_index_tree, pre_call_index_tree,
        "pre_commit_with_tree must restore the original index state after running the hook"
    );
    Ok(())
}

/// When a hook stages a brand-new file (one that did not exist in the
/// committed tree), the returned `post_hook_tree` must contain that new file.
#[test]
#[cfg(unix)]
fn pre_commit_with_tree_hook_stages_new_file() -> Result<()> {
    let (ctx, _tmp, tree_oid) = setup_repo_with_commit("existing content\n")?;

    let workdir = ctx.repo.get()?.workdir().unwrap().to_owned();
    let repo = git2::Repository::open(&workdir)?;
    install_hook(
        &repo,
        "pre-commit",
        "#!/bin/sh\nprintf 'new file content\\n' > added_by_hook.txt\ngit add added_by_hook.txt\n",
    )?;

    let (result, post_hook_tree) = pre_commit_with_tree(&ctx, tree_oid)?;

    assert_eq!(result, HookResult::Success);
    assert_ne!(
        post_hook_tree, tree_oid,
        "post-hook tree should differ when hook stages a new file"
    );

    let git2_repo = ctx.git2_repo.get()?;
    let post_tree = git2_repo.find_tree(post_hook_tree)?;
    assert!(
        post_tree.get_name("added_by_hook.txt").is_some(),
        "post-hook tree must contain the new file staged by the hook"
    );
    // Original file must also still be present.
    assert!(
        post_tree.get_name("file.txt").is_some(),
        "original file must still be present in post-hook tree"
    );
    Ok(())
}

/// When the hook exits with a non-zero status the function must return
/// `HookResult::Failure` and the post-hook tree must still equal the input
/// tree (since a failing hook should not affect what gets committed).
#[test]
#[cfg(unix)]
fn pre_commit_with_tree_hook_failure() -> Result<()> {
    let (ctx, _tmp, tree_oid) = setup_repo_with_commit("content\n")?;

    let repo = git2::Repository::open(ctx.repo.get()?.workdir().unwrap())?;
    install_hook(
        &repo,
        "pre-commit",
        "#!/bin/sh\necho 'hook failed' >&2\nexit 1\n",
    )?;

    let (result, post_hook_tree) = pre_commit_with_tree(&ctx, tree_oid)?;

    assert!(
        matches!(result, HookResult::Failure(_)),
        "hook exiting 1 must produce HookResult::Failure"
    );
    assert_eq!(
        post_hook_tree, tree_oid,
        "post-hook tree should equal the input tree when the hook fails (no changes committed)"
    );
    Ok(())
}
