// SPDX-License-Identifier: AGPL-3.0-or-later
import { useCallback, useEffect, useRef } from "react";

const shortcuts = [
	{ section: "Navigation" },
	{ keys: "Alt+\u2190", action: "Go back" },
	{ keys: "Alt+\u2192", action: "Go forward" },
	{ section: "View" },
	{ keys: "Ctrl++", action: "Zoom in" },
	{ keys: "Ctrl+\u2013", action: "Zoom out" },
	{ keys: "Ctrl+0", action: "Reset zoom" },
] as const;

export default function KeyboardShortcutsModal({
	open,
	onClose,
}: {
	open: boolean;
	onClose: () => void;
}) {
	const dialogRef = useRef<HTMLDialogElement>(null);

	useEffect(() => {
		if (open) {
			dialogRef.current?.showModal();
		} else {
			dialogRef.current?.close();
		}
	}, [open]);

	const handleBackdropClick = useCallback(
		(e: React.MouseEvent) => {
			if (e.target === dialogRef.current) onClose();
		},
		[onClose],
	);

	return (
		<dialog
			ref={dialogRef}
			className="modal"
			onClose={onClose}
			onClick={handleBackdropClick}
		>
			<div className="modal-box max-w-md">
				<h3 className="font-bold text-lg mb-4">Keyboard Shortcuts</h3>
				<div className="space-y-1">
					{shortcuts.map((item, i) =>
						"section" in item ? (
							<div
								key={item.section}
								className={`text-xs font-semibold text-base-content/50 uppercase tracking-wider ${i > 0 ? "pt-3" : ""}`}
							>
								{item.section}
							</div>
						) : (
							<div
								key={item.keys}
								className="flex items-center justify-between py-1"
							>
								<span className="text-sm text-base-content/80">
									{item.action}
								</span>
								<kbd className="kbd kbd-sm">{item.keys}</kbd>
							</div>
						),
					)}
				</div>
				<div className="modal-action">
					<button
						type="button"
						className="btn btn-sm btn-ghost"
						onClick={onClose}
					>
						Close
					</button>
				</div>
			</div>
		</dialog>
	);
}
