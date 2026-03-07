import { parseError } from "$lib/error/parser";
import { hasBackendExtra } from "$lib/state/backendQuery";
import { invalidatesList, providesList, ReduxTag } from "$lib/state/tags";
import { InjectionToken } from "@gitbutler/core/context";
import type { BackendApi } from "$lib/state/backendApi";

export const MODE_SERVICE = new InjectionToken<ModeService>("ModeService");

export class ModeService {
	constructor(private backendApi: BackendApi) {}

	get enterEditMode() {
		return this.backendApi.endpoints.enterEditMode.mutate;
	}

	get abortEditAndReturnToWorkspace() {
		return this.backendApi.endpoints.abortEditAndReturnToWorkspace.mutate;
	}

	get abortEditAndReturnToWorkspaceMutation() {
		return this.backendApi.endpoints.abortEditAndReturnToWorkspace.useMutation();
	}

	get saveEditAndReturnToWorkspace() {
		return this.backendApi.endpoints.saveEditAndReturnToWorkspace.mutate;
	}

	get saveEditAndReturnToWorkspaceMutation() {
		return this.backendApi.endpoints.saveEditAndReturnToWorkspace.useMutation();
	}

	get initialEditModeState() {
		return this.backendApi.endpoints.initialEditModeState.useQuery;
	}

	get changesSinceInitialEditState() {
		return this.backendApi.endpoints.changesSinceInitialEditState.useQuery;
	}

	mode(projectId: string) {
		return this.backendApi.endpoints.headAndMode.useQuery(
			{ projectId },
			{ transform: (response) => response.operatingMode },
		);
	}

	/**
	 * Force-fetch the current mode, bypassing the cache. This updates the
	 * cache so that reactive subscribers see the new value immediately.
	 */
	async fetchMode(projectId: string) {
		return await this.backendApi.endpoints.headAndMode.fetch({ projectId }, { forceRefetch: true });
	}

	head(projectId: string) {
		return this.backendApi.endpoints.headSha.useQuery(
			{ projectId },
			{ transform: (response) => response.headSha },
		);
	}
}

function injectEndpoints(api: BackendApi) {
	return api.injectEndpoints({
		endpoints: (build) => ({
			enterEditMode: build.mutation<void, { projectId: string; commitId: string; stackId: string }>(
				{
					extraOptions: { command: "enter_edit_mode" },
					query: (args) => args,
					invalidatesTags: [
						invalidatesList(ReduxTag.InitalEditListing),
						invalidatesList(ReduxTag.EditChangesSinceInitial),
						invalidatesList(ReduxTag.HeadMetadata),
					],
				},
			),
			abortEditAndReturnToWorkspace: build.mutation<void, { projectId: string; force: boolean }>({
				extraOptions: { command: "abort_edit_and_return_to_workspace" },
				query: (args) => args,
				invalidatesTags: [invalidatesList(ReduxTag.HeadMetadata)],
			}),
			saveEditAndReturnToWorkspace: build.mutation<void, { projectId: string }>({
				extraOptions: { command: "save_edit_and_return_to_workspace" },
				query: (args) => args,
				invalidatesTags: [
					invalidatesList(ReduxTag.WorktreeChanges),
					invalidatesList(ReduxTag.HeadSha),
					invalidatesList(ReduxTag.HeadMetadata),
				],
			}),
			initialEditModeState: build.query<
				[TreeChange, ConflictEntryPresence | undefined][],
				{ projectId: string }
			>({
				extraOptions: { command: "edit_initial_index_state" },
				query: (args) => args,
				providesTags: [providesList(ReduxTag.InitalEditListing)],
			}),
			changesSinceInitialEditState: build.query<TreeChange[], { projectId: string }>({
				extraOptions: { command: "edit_changes_from_initial" },
				query: (args) => args,
				providesTags: [providesList(ReduxTag.EditChangesSinceInitial)],
				async onCacheEntryAdded(arg, lifecycleApi) {
					if (!hasBackendExtra(lifecycleApi.extra)) {
						throw new Error("Redux dependency Backend not found!");
					}
					const { invoke, listen } = lifecycleApi.extra.backend;
					await lifecycleApi.cacheDataLoaded;
					let finished = false;
					// We are listening to this only for the notification that changes have been made
					const unsubscribe = listen<unknown>(
						`project://${arg.projectId}/worktree_changes`,
						async (_) => {
							if (finished) return;
							try {
								const changes = await invoke<TreeChange[]>("edit_changes_from_initial", arg);
								lifecycleApi.updateCachedData(() => changes);
							} catch (error: unknown) {
								// Edit mode may have been exited (e.g. via CLI) before this
								// listener was unsubscribed. Silently ignore that specific race.
								const { message } = parseError(error);
								if (!message.includes("Expected to be in edit mode")) {
									throw error;
								}
							}
						},
					);
					// The `cacheEntryRemoved` promise resolves when the result is removed
					await lifecycleApi.cacheEntryRemoved;
					finished = true;
					unsubscribe();
				},
			}),
			headAndMode: build.query<HeadAndMode, { projectId: string }>({
				extraOptions: { command: "operating_mode" },
				query: (args) => args,
				providesTags: [providesList(ReduxTag.HeadMetadata)],
				async onCacheEntryAdded(arg, lifecycleApi) {
					if (!hasBackendExtra(lifecycleApi.extra)) {
						throw new Error("Redux dependency Backend not found!");
					}
					await lifecycleApi.cacheDataLoaded;
					const unsubscribe = lifecycleApi.extra.backend.listen<HeadAndMode>(
						`project://${arg.projectId}/git/head`,
						(event) => {
							lifecycleApi.updateCachedData(() => event.payload);
						},
					);
					await lifecycleApi.cacheEntryRemoved;
					unsubscribe();
				},
			}),
			headSha: build.query<HeadSha, { projectId: string }>({
				extraOptions: { command: "head_sha" },
				query: (args) => args,
				providesTags: [providesList(ReduxTag.HeadSha)],
				async onCacheEntryAdded(arg, lifecycleApi) {
					if (!hasBackendExtra(lifecycleApi.extra)) {
						throw new Error("Redux dependency Backend not found!");
					}
					await lifecycleApi.cacheDataLoaded;
					const unsubscribe = lifecycleApi.extra.backend.listen<HeadSha>(
						`project://${arg.projectId}/git/activity`,
						(event) => {
							lifecycleApi.updateCachedData(() => event.payload);
						},
					);
					await lifecycleApi.cacheEntryRemoved;
					unsubscribe();
				},
			}),
		}),
	});
}
