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
 * Result of the pre-commit diffspec hook. In addition to the standard hook
 * status, the backend may include `updatedChanges` when the hook staged new
 * files or modifications beyond the user's original selection.  Callers
 * should use `updatedChanges` (when non-empty) instead of the original
 * changes when creating the commit.
 *
 * Note: `updatedChanges` only appears on the `"success"` variant — if no
 * hook is configured the index is untouched so there can never be updates.
 */
export type PreCommitHookDiffspecsStatus =
	| {
			status: "success";
			/** Non-empty only when the hook staged additional changes. */
			updatedChanges?: DiffSpec[];
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
	 * Returns the (possibly updated) list of diff specs to use for the
	 * commit.  If the hook staged new changes the returned list will differ
	 * from the input `changes`; callers **must** use the returned list when
	 * creating the commit so that hook-staged modifications are included.
	 */
	async runPreCommitHooks(projectId: string, changes: DiffSpec[]): Promise<DiffSpec[]> {
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

			// Return the updated changes if the hook staged new files/modifications,
			// otherwise return the original changes unchanged.
			if (result?.updatedChanges?.length) {
				return result.updatedChanges;
			}
			return changes;
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
