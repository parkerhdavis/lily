// SPDX-License-Identifier: AGPL-3.0-or-later
import type { DocumentDiffLine } from "@/types";

/** A unified paragraph diff. Lines marked − are from the old document; lines
 *  marked + are from the new one. */
export default function DiffView({
	diff,
	emptyMessage = "No changes.",
}: {
	diff: DocumentDiffLine[];
	emptyMessage?: string;
}) {
	return (
		<div className="max-h-[60vh] overflow-auto rounded border border-base-300 bg-base-100 p-3 font-mono text-xs">
			{diff.map((line, index) => (
				<div
					key={`${index}-${line.kind}`}
					className={
						line.kind === "added"
							? "bg-success/10 text-success"
							: line.kind === "removed"
								? "bg-error/10 text-error"
								: "text-base-content/70"
					}
				>
					<span className="inline-block w-4 select-none">
						{line.kind === "added" ? "+" : line.kind === "removed" ? "−" : " "}
					</span>
					<span className="whitespace-pre-wrap break-words">{line.text}</span>
				</div>
			))}
			{diff.length === 0 && (
				<p className="text-base-content/50">{emptyMessage}</p>
			)}
		</div>
	);
}
