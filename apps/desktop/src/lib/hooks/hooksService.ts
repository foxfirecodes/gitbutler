import { InjectionToken } from "@gitbutler/core/context";
import { chipToasts } from "@gitbutler/ui";
import type { DiffSpec } from "$lib/hunks/hunk";
import type { BackendApi } from "$lib/state/clientState.svelte";

export type HookStatus =
	| {
			status: "success";
	  }
	| {
			status: "notconfigured";
	  }
	| {
			status: "failure";
			error: string;
	  };

/**
 * Result of the pre-commit diffspec hook.
 *
 * When the hook modified the git index (including via partial file staging),
 * the `"success"` variant includes `postHookTree` — the hex OID of the tree
 * written from the post-hook index.  Callers **must** pass this to
 * `commit_create` / `commit_amend` as `overrideTree` so the commit engine
 * uses the tree directly instead of re-reading worktree files (which would
 * discard partial staging).
 *
 * Note: `postHookTree` only appears on `"success"` — if no hook is configured
 * the index is untouched so there can never be an updated tree.
 */
export type PreCommitHookDiffspecsStatus =
	| {
			status: "success";
			/** Hex OID of post-hook index tree. Present only when the hook modified the index. */
			postHookTree?: string;
	  }
	| {
			status: "notconfigured";
	  }
	| {
			status: "failure";
			error: string;
	  };

export type MessageHookStatus =
	| {
			status: "success";
	  }
	| {
			status: "message";
			message: string;
	  }
	| {
			status: "notconfigured";
	  }
	| {
			status: "failure";
			error: string;
	  };

export const HOOKS_SERVICE = new InjectionToken<HooksService>("HooksService");

export class HooksService {
	private api: ReturnType<typeof injectEndpoints>;

	constructor(backendApi: BackendApi) {
		this.api = injectEndpoints(backendApi);
	}

	get message() {
		return this.api.endpoints.message.useMutation();
	}

	/**
	 * Run the pre-commit hooks for the given changes.
	 *
	 * Returns the `postHookTree` hex OID when the hook modified the git index
	 * (including via partial file staging).  The caller must pass this as
	 * `overrideTree` to `commit_create` / `commit_amend` so the commit engine
	 * uses the hook's staged tree directly.
	 *
	 * Returns `undefined` when no hook ran or the hook did not change the index;
	 * in that case the caller should use its original DiffSpecs.
	 */
	async runPreCommitHooks(projectId: string, changes: DiffSpec[]): Promise<string | undefined> {
		const loadingToastId = chipToasts.loading("Started pre-commit hooks");

		try {
			const result = await this.api.endpoints.preCommitDiffspecs.mutate({
				projectId,
				changes,
			});

			if (result?.status === "failure") {
				chipToasts.removeChipToast(loadingToastId);
				throw new Error(formatError(result.error));
			}

			chipToasts.removeChipToast(loadingToastId);
			chipToasts.success("Pre-commit hooks succeeded");

			// Return the post-hook tree OID when the hook staged changes (including
			// partial staging).  Callers must use it as `overrideTree` to avoid
			// re-reading worktree files and losing partial staging.
			return result?.status === "success" ? result.postHookTree : undefined;
		} catch (e: unknown) {
			chipToasts.removeChipToast(loadingToastId);
			throw e;
		}
	}

	async runPostCommitHooks(projectId: string): Promise<void> {
		const loadingToastId = chipToasts.loading("Started post-commit hooks");

		try {
			const result = await this.api.endpoints.postCommit.mutate({
				projectId,
			});

			if (result?.status === "failure") {
				chipToasts.removeChipToast(loadingToastId);
				throw new Error(formatError(result.error));
			}

			chipToasts.removeChipToast(loadingToastId);
			chipToasts.success("Post-commit hooks succeeded");
		} catch (e: unknown) {
			chipToasts.removeChipToast(loadingToastId);
			throw e;
		}
	}
}

function formatError(error: string): string {
	return `${error}\n\nIf you don't want git hooks to run, disable "Run Git hooks" in project settings.`;
}

function injectEndpoints(backendApi: BackendApi) {
	return backendApi.injectEndpoints({
		endpoints: (build) => ({
			preCommitDiffspecs: build.mutation<
				PreCommitHookDiffspecsStatus,
				{ projectId: string; changes: DiffSpec[] }
			>({
				extraOptions: { command: "pre_commit_hook_diffspecs" },
				query: (args) => args,
			}),
			postCommit: build.mutation<HookStatus, { projectId: string }>({
				extraOptions: { command: "post_commit_hook" },
				query: (args) => args,
			}),
			message: build.mutation<MessageHookStatus, { projectId: string; message: string }>({
				extraOptions: { command: "message_hook" },
				query: (args) => args,
			}),
		}),
	});
}
