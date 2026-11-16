// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { create } from "zustand";
import type { AppSettings } from "@/types";

const MAX_RECENT_DIRECTORIES = 10;

interface SettingsState {
	settings: AppSettings;
	loaded: boolean;
	load: () => Promise<void>;
	save: (settings: Partial<AppSettings>) => Promise<void>;
	addRecentDirectory: (dir: string) => Promise<void>;
	removeRecentDirectory: (dir: string) => Promise<void>;
	toggleTheme: () => Promise<void>;
	zoomIn: () => Promise<void>;
	zoomOut: () => Promise<void>;
	zoomReset: () => Promise<void>;
}

export const useSettingsStore = create<SettingsState>((set, get) => ({
	settings: {
		templates_dir: null,
		last_working_dir: null,
		recent_directories: [],
		window_width: null,
		window_height: null,
		theme: null,
		zoom: null,
		footer_size: null,
		last_step: null,
		last_template_rel_path: null,
		autosave: null,
		questionnaires_dir: null,
		client_library_dirs: [],
		navigation_history: [],
		agent_command: null,
		agent_model: null,
		agent_repo_dir: null,
	},
	loaded: false,

	load: async () => {
		try {
			const settings = await invoke<AppSettings>("load_settings");
			applyTheme(settings.theme);
			applyZoom(settings.zoom);
			set({ settings, loaded: true });
		} catch (err) {
			console.error("Failed to load settings:", err);
			set({ loaded: true });
		}
	},

	save: async (partial) => {
		const current = get().settings;
		const updated = { ...current, ...partial };
		try {
			await invoke("save_settings", { settings: updated });
			set({ settings: updated });
		} catch (err) {
			console.error("Failed to save settings:", err);
		}
	},

	addRecentDirectory: async (dir) => {
		const current = get().settings;
		const filtered = current.recent_directories.filter((d) => d !== dir);
		const recent = [dir, ...filtered].slice(0, MAX_RECENT_DIRECTORIES);
		await get().save({ recent_directories: recent });
	},

	removeRecentDirectory: async (dir) => {
		const current = get().settings;
		const filtered = current.recent_directories.filter((d) => d !== dir);
		await get().save({ recent_directories: filtered });
	},

	toggleTheme: async () => {
		const current = get().settings;
		const next = current.theme === "dark" ? "light" : "dark";
		applyTheme(next);
		await get().save({ theme: next });
	},

	zoomIn: async () => {
		const current = get().settings.zoom ?? 100;
		const next = Math.min(current + 5, 200);
		applyZoom(next);
		await get().save({ zoom: next });
	},

	zoomOut: async () => {
		const current = get().settings.zoom ?? 100;
		const next = Math.max(current - 5, 50);
		applyZoom(next);
		await get().save({ zoom: next });
	},

	zoomReset: async () => {
		applyZoom(100);
		await get().save({ zoom: 100 });
	},
}));

/** Set the daisyUI data-theme attribute on the root <html> element. */
function applyTheme(theme: string | null) {
	document.documentElement.setAttribute(
		"data-theme",
		theme === "dark" ? "dark" : "light",
	);
}

/**
 * Apply zoom level. The actual CSS zoom is applied via React inline style
 * on the page content wrapper (not here), so the status bar stays unaffected.
 * This function clears any stale zoom on the root element from older versions.
 */
function applyZoom(_zoom: number | null) {
	document.documentElement.style.zoom = "";
}
