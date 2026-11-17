// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import AppSettings from "@/components/AppSettings";
import ClientsHub from "@/components/ClientsHub";
import LilyHub from "@/components/LilyHub";
import PipelineHub from "@/components/PipelineHub";
import TemplatePicker from "@/components/TemplatePicker";
import ErrorBoundary from "@/components/ui/ErrorBoundary";
import KeyboardShortcutsModal from "@/components/ui/KeyboardShortcutsModal";
import StatusBar from "@/components/ui/StatusBar";
import ToastContainer from "@/components/ui/ToastContainer";
import { useLilyIcon } from "@/hooks/useLilyIcon";
import { useNavigationStore } from "@/stores/navigationStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useWorkflowStore } from "@/stores/workflowStore";

export default function App() {
	const { loaded, load, zoomIn, zoomOut, zoomReset } = useSettingsStore();
	const zoom = useSettingsStore((s) => s.settings.zoom) ?? 100;
	const step = useWorkflowStore((s) => s.step);
	const [splashDone, setSplashDone] = useState(false);
	const [fadeOut, setFadeOut] = useState(false);
	const [revealed, setRevealed] = useState(false);
	const [showShortcuts, setShowShortcuts] = useState(false);
	const lilyIcon = useLilyIcon();

	useEffect(() => {
		load();
	}, [load]);

	// Reveal the main window once React has committed the splash. The window is
	// created hidden (tauri.conf.json `visible: false`) so the OS never shows the
	// unpainted webview, the static index.html fallback, or the live `set_size`
	// resize in Rust `.setup()` — its first on-screen frame is the splash.
	//
	// Scheduled with setTimeout, NOT requestAnimationFrame: webkit2gtk does not
	// service rAF while the window is hidden (nothing is being composited), so an
	// rAF-gated reveal would never fire. A Rust-side safety timer reveals the
	// window anyway if this never runs at all.
	useEffect(() => {
		const t = setTimeout(() => {
			setRevealed(true);
			void invoke("show_main_window").catch(() => {});
		}, 0);
		return () => clearTimeout(t);
	}, []);

	// Track the current step in settings so LilyHub can offer "pick up where you left off"
	// Only save non-hub steps — landing on hub at startup should not clear the previous value.
	useEffect(() => {
		if (!loaded || step === "hub") return;
		useSettingsStore.getState().save({ last_step: step });
	}, [step, loaded]);

	// Global zoom keyboard shortcuts: Ctrl+= / Ctrl+- / Ctrl+0
	useEffect(() => {
		const handleZoom = (e: KeyboardEvent) => {
			if (!(e.ctrlKey || e.metaKey)) return;
			if (e.key === "=" || e.key === "+") {
				e.preventDefault();
				zoomIn();
			} else if (e.key === "-") {
				e.preventDefault();
				zoomOut();
			} else if (e.key === "0") {
				e.preventDefault();
				zoomReset();
			}
		};
		window.addEventListener("keydown", handleZoom);
		return () => window.removeEventListener("keydown", handleZoom);
	}, [zoomIn, zoomOut, zoomReset]);

	// Global navigation shortcuts: Alt+Left / Alt+Right
	useEffect(() => {
		const handleNav = (e: KeyboardEvent) => {
			if (!e.altKey) return;
			if (e.key === "ArrowLeft") {
				e.preventDefault();
				const entry = useNavigationStore.getState().goBack();
				if (entry) useWorkflowStore.getState().restoreNavigationEntry(entry);
			} else if (e.key === "ArrowRight") {
				e.preventDefault();
				const entry = useNavigationStore.getState().goForward();
				if (entry) useWorkflowStore.getState().restoreNavigationEntry(entry);
			}
		};
		window.addEventListener("keydown", handleNav);
		return () => window.removeEventListener("keydown", handleNav);
	}, []);

	// Splash: hold until settings have loaded AND the window is actually on-screen,
	// then run the minimum display time before fading out. Gating on `revealed`
	// (not just `loaded`) keeps the splash's full dwell measured from when it is
	// first visible, regardless of how long the webview warmed up while hidden.
	useEffect(() => {
		if (!loaded || !revealed) return;
		const fadeTimer = setTimeout(() => setFadeOut(true), 1200);
		const doneTimer = setTimeout(() => setSplashDone(true), 1500);
		return () => {
			clearTimeout(fadeTimer);
			clearTimeout(doneTimer);
		};
	}, [loaded, revealed]);

	if (!splashDone) {
		return (
			<div
				className={`flex flex-col items-center justify-center min-h-screen gap-3 transition-opacity duration-300 ${fadeOut ? "opacity-0" : "opacity-100"}`}
			>
				<img
					src={lilyIcon}
					alt="Loading Lily..."
					className="size-24 animate-lily-spin"
				/>
				<span className="text-3xl font-bold tracking-tight animate-fade-in-up">
					Lily
				</span>
			</div>
		);
	}

	const page = (() => {
		switch (step) {
			case "hub":
				return <LilyHub />;
			case "clients":
				return <ClientsHub />;
			case "select-template":
				return <TemplatePicker />;
			case "app-settings":
				return <AppSettings />;
			case "pipeline":
				return <PipelineHub />;
		}
	})();

	return (
		<div className="flex flex-col h-screen">
			<div
				className="flex-1 min-h-0"
				style={zoom !== 100 ? { zoom: `${zoom}%` } : undefined}
			>
				<ErrorBoundary>{page}</ErrorBoundary>
			</div>
			<StatusBar onShowShortcuts={() => setShowShortcuts(true)} />
			<ToastContainer />
			<KeyboardShortcutsModal
				open={showShortcuts}
				onClose={() => setShowShortcuts(false)}
			/>
		</div>
	);
}
