use std::path::PathBuf;

use anyhow::{Context as _, Result};
use bstr::BString;
use but_api_macros::but_api;
use but_core::DiffSpec;
use but_ctx::Context;
use but_oxidize::ObjectIdExt;
use gitbutler_branch_actions::hooks;
use gitbutler_repo::{
    FileInfo, RepoCommands,
    hooks::{HookResult, MessageHookResult, PreCommitHookDiffspecsResult},
};
use gitbutler_repo_actions::askpass;
use tracing::instrument;

#[but_api]
#[instrument(err(Debug))]
pub fn check_signing_settings(ctx: &Context) -> Result<bool> {
    ctx.check_signing_settings()
}

/// NOTE: this function currently needs a tokio runtime to work.
#[but_api]
#[instrument(err(Debug))]
pub async fn git_clone_repository(repository_url: String, target_dir: PathBuf) -> Result<()> {
    let handle_prompt = if askpass::get_broker().is_some() {
        Some(|prompt: String| handle_git_prompt_clone(prompt, repository_url.clone()))
    } else {
        None
    };

    gitbutler_git::clone(
        &repository_url,
        &target_dir,
        gitbutler_git::tokio::TokioExecutor,
        handle_prompt,
    )
    .await?;
    Ok(())
}

async fn handle_git_prompt_clone(prompt: String, url: String) -> Option<String> {
    tracing::info!("received prompt for clone of {url}: {prompt:?}");
    askpass::get_broker()
        .expect("askpass broker must be initialized")
        .submit_prompt(prompt, askpass::Context::Clone { url })
        .await
}

#[but_api]
#[instrument(err(Debug))]
pub fn get_commit_file(
    ctx: &Context,
    relative_path: PathBuf,
    commit_id: gix::ObjectId,
) -> Result<FileInfo> {
    ctx.read_file_from_commit(commit_id.to_git2(), &relative_path)
}

#[but_api]
#[instrument(err(Debug))]
pub fn get_workspace_file(ctx: &Context, relative_path: PathBuf) -> Result<FileInfo> {
    ctx.read_file_from_workspace(&relative_path)
}

/// Retrieves file content directly from a Git blob object by its blob ID.
///
/// This function is used for displaying image diff previews when the file
/// isn't available in the current workspace or a specific commit (e.g., for
/// deleted files or when comparing against a previous state).
///
/// # Arguments
/// * `blob_id` - Git blob object ID as a hexadecimal string
#[but_api]
#[instrument(err(Debug))]
pub fn get_blob_file(
    ctx: &but_ctx::Context,
    relative_path: PathBuf,
    blob_id: gix::ObjectId,
) -> Result<FileInfo> {
    let repo = ctx.repo.get()?;
    let object = repo.find_object(blob_id).context("Failed to find blob")?;
    let blob = object.try_into_blob().context("Object is not a blob")?;
    Ok(FileInfo::from_content(&relative_path, &blob.data))
}

#[but_api]
#[instrument(err(Debug))]
pub fn pre_commit_hook_diffspecs(
    ctx: &but_ctx::Context,
    changes: Vec<DiffSpec>,
) -> Result<PreCommitHookDiffspecsResult> {
    let repo = ctx.repo.get()?;
    let head = repo
        .head_tree_id_or_empty()
        .context("Failed to get head tree")?;

    let context_lines = ctx.settings.context_lines;

    let mut changes_for_tree = changes.clone().into_iter().map(Ok).collect::<Vec<_>>();

    let (new_tree, ..) = but_core::tree::apply_worktree_changes(
        head.detach(),
        &repo,
        &mut changes_for_tree,
        context_lines,
    )?;

    let new_tree_git2 = new_tree.to_git2();
    let (hook_result, post_hook_tree) = hooks::pre_commit_with_tree(ctx, new_tree_git2)?;

    // If the hook succeeded and modified the index (staged new changes), compute
    // the updated diff specs so the caller can include those changes in the commit.
    // NotConfigured means no hook ran, so the index is unchanged — no updated changes possible.
    let updated_changes = if matches!(hook_result, HookResult::Success)
        && post_hook_tree != new_tree_git2
    {
        let git2_repo = ctx.git2_repo.get()?;
        compute_hook_updated_changes(&git2_repo, changes, new_tree_git2, post_hook_tree)?
    } else {
        vec![]
    };

    Ok(match hook_result {
        HookResult::Success => PreCommitHookDiffspecsResult::Success { updated_changes },
        HookResult::NotConfigured => PreCommitHookDiffspecsResult::NotConfigured,
        HookResult::Failure(err) => PreCommitHookDiffspecsResult::Failure(err),
    })
}

/// Given the diff between the tree that was passed to the pre-commit hook (`original_tree`)
/// and the tree the hook produced (`post_hook_tree`), return an updated set of [`DiffSpec`]s
/// that incorporates both the user's original selection and any files the hook additionally
/// staged.  Files touched by the hook are committed as whole files (empty `hunk_headers`).
fn compute_hook_updated_changes(
    repo: &git2::Repository,
    original_changes: Vec<DiffSpec>,
    original_tree: git2::Oid,
    post_hook_tree: git2::Oid,
) -> Result<Vec<DiffSpec>> {
    let old_tree = repo.find_tree(original_tree)?;
    let new_tree = repo.find_tree(post_hook_tree)?;
    let diff = repo.diff_tree_to_tree(Some(&old_tree), Some(&new_tree), None)?;

    let mut hook_changed_paths: std::collections::HashSet<BString> =
        std::collections::HashSet::new();
    let mut hook_added_specs: Vec<DiffSpec> = Vec::new();

    diff.foreach(
        &mut |delta, _| {
            let (path, previous_path) = match delta.status() {
                git2::Delta::Deleted => (
                    delta
                        .old_file()
                        .path()
                        .map(|p| BString::from(p.as_os_str().as_encoded_bytes())),
                    None,
                ),
                git2::Delta::Renamed => (
                    delta
                        .new_file()
                        .path()
                        .map(|p| BString::from(p.as_os_str().as_encoded_bytes())),
                    delta
                        .old_file()
                        .path()
                        .map(|p| BString::from(p.as_os_str().as_encoded_bytes())),
                ),
                _ => (
                    delta
                        .new_file()
                        .path()
                        .map(|p| BString::from(p.as_os_str().as_encoded_bytes())),
                    None,
                ),
            };

            if let Some(path) = path {
                hook_changed_paths.insert(path.clone());
                hook_added_specs.push(DiffSpec {
                    path,
                    previous_path,
                    // Empty hunk_headers means "commit the whole file", which is correct
                    // because the hook staged these changes from the worktree.
                    hunk_headers: vec![],
                });
            }
            true
        },
        None,
        None,
        None,
    )?;

    // Preserve the user's original hunk-level selections for files the hook did not touch,
    // and replace with whole-file specs for any file the hook modified.
    let mut result: Vec<DiffSpec> = original_changes
        .into_iter()
        .filter(|spec| !hook_changed_paths.contains(&spec.path))
        .collect();
    result.extend(hook_added_specs);
    Ok(result)
}
#[but_api]
#[instrument(err(Debug))]
pub fn post_commit_hook(ctx: &but_ctx::Context) -> Result<HookResult> {
    gitbutler_repo::hooks::post_commit(ctx)
}

#[but_api]
#[instrument(err(Debug))]
pub fn message_hook(ctx: &but_ctx::Context, message: String) -> Result<MessageHookResult> {
    gitbutler_repo::hooks::commit_msg(ctx, message)
}

#[but_api]
#[instrument(err(Debug))]
pub fn find_files(ctx: &Context, query: String, limit: Option<usize>) -> Result<Vec<String>> {
    let limit = limit.unwrap_or(10);
    ctx.find_files(&query, limit)
}
