// SPDX-License-Identifier: AGPL-3.0-or-later
import { useCallback, useEffect, useRef, useState } from "react";
import PageHeader from "@/components/ui/PageHeader";
import SectionHeading from "@/components/ui/SectionHeading";
import { useQuestionnaireStore } from "@/stores/questionnaireStore";
import { useSettingsStore } from "@/stores/settingsStore";
import { useWorkflowStore } from "@/stores/workflowStore";
import type {
	QuestionDef,
	QuestionnaireDefFile,
	QuestionnaireSectionDef,
} from "@/types/questionnaire";

// ─── Main component ─────────────────────────────────────────────────────────

export default function QuestionnaireEditor() {
	const goToPipeline = useWorkflowStore((s) => s.goToPipeline);
	const { currentDef, saveQuestionnaire } = useQuestionnaireStore();
	const autosave = useSettingsStore((s) => s.settings.autosave) !== false;

	const [def, setDef] = useState<QuestionnaireDefFile | null>(null);
	const [activeTab, setActiveTab] = useState<string>("");
	const [editingName, setEditingName] = useState(false);
	const [nameValue, setNameValue] = useState("");
	const saveTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
	const [saving, setSaving] = useState(false);
	const [saved, setSaved] = useState(false);
	const [dirty, setDirty] = useState(false);

	// Initialize from currentDef
	useEffect(() => {
		if (currentDef) {
			setDef(currentDef);
			if (currentDef.tabs.length > 0) {
				setActiveTab(currentDef.tabs[0].id);
			}
		}
	}, [currentDef]);

	// Save with debounce (auto or manual trigger)
	const doSave = useCallback(
		async (updated: QuestionnaireDefFile) => {
			setSaving(true);
			setSaved(false);
			await saveQuestionnaire(updated);
			setSaving(false);
			setSaved(true);
			setDirty(false);
			setTimeout(() => setSaved(false), 2000);
		},
		[saveQuestionnaire],
	);

	const scheduleSave = useCallback(
		(updated: QuestionnaireDefFile) => {
			if (!autosave) {
				setDirty(true);
				return;
			}
			if (saveTimer.current) clearTimeout(saveTimer.current);
			saveTimer.current = setTimeout(() => doSave(updated), 600);
		},
		[autosave, doSave],
	);

	const updateDef = useCallback(
		(updater: (prev: QuestionnaireDefFile) => QuestionnaireDefFile) => {
			setDef((prev) => {
				if (!prev) return prev;
				const updated = updater(prev);
				scheduleSave(updated);
				return updated;
			});
		},
		[scheduleSave],
	);

	// Ctrl+S manual save
	useEffect(() => {
		const handleKeyDown = (e: KeyboardEvent) => {
			if ((e.ctrlKey || e.metaKey) && e.key === "s") {
				e.preventDefault();
				if (def && dirty) doSave(def);
			}
		};
		window.addEventListener("keydown", handleKeyDown);
		return () => window.removeEventListener("keydown", handleKeyDown);
	}, [def, dirty, doSave]);

	// Warn about unsaved changes when closing the window
	useEffect(() => {
		if (autosave || !dirty) return;
		const handler = (e: BeforeUnloadEvent) => {
			e.preventDefault();
		};
		window.addEventListener("beforeunload", handler);
		return () => window.removeEventListener("beforeunload", handler);
	}, [autosave, dirty]);

	if (!def) {
		return (
			<div className="flex items-center justify-center h-full">
				<span className="loading loading-spinner loading-md" />
			</div>
		);
	}

	const tabSections = def.sections.filter((s) => s.tab === activeTab);

	const handleNameSubmit = () => {
		if (nameValue.trim() && nameValue.trim() !== def.name) {
			updateDef((d) => ({ ...d, name: nameValue.trim() }));
		}
		setEditingName(false);
	};

	const addSection = () => {
		updateDef((d) => ({
			...d,
			sections: [
				...d.sections,
				{
					title: "New Section",
					tab: activeTab as QuestionnaireSectionDef["tab"],
					description: "",
					questions: [],
				},
			],
		}));
	};

	const updateSection = (
		sectionIndex: number,
		updater: (s: QuestionnaireSectionDef) => QuestionnaireSectionDef,
	) => {
		updateDef((d) => {
			// Find the absolute index within d.sections for this tab section
			const tabIndices = d.sections
				.map((s, i) => (s.tab === activeTab ? i : -1))
				.filter((i) => i >= 0);
			const absIndex = tabIndices[sectionIndex];
			if (absIndex === undefined) return d;

			const sections = [...d.sections];
			sections[absIndex] = updater(sections[absIndex]);
			return { ...d, sections };
		});
	};

	const removeSection = (sectionIndex: number) => {
		updateDef((d) => {
			const tabIndices = d.sections
				.map((s, i) => (s.tab === activeTab ? i : -1))
				.filter((i) => i >= 0);
			const absIndex = tabIndices[sectionIndex];
			if (absIndex === undefined) return d;

			const sections = d.sections.filter((_, i) => i !== absIndex);
			return { ...d, sections };
		});
	};

	const moveSectionUp = (sectionIndex: number) => {
		if (sectionIndex === 0) return;
		updateDef((d) => {
			const tabIndices = d.sections
				.map((s, i) => (s.tab === activeTab ? i : -1))
				.filter((i) => i >= 0);
			const absA = tabIndices[sectionIndex];
			const absB = tabIndices[sectionIndex - 1];
			if (absA === undefined || absB === undefined) return d;

			const sections = [...d.sections];
			[sections[absA], sections[absB]] = [sections[absB], sections[absA]];
			return { ...d, sections };
		});
	};

	const moveSectionDown = (sectionIndex: number) => {
		if (sectionIndex >= tabSections.length - 1) return;
		updateDef((d) => {
			const tabIndices = d.sections
				.map((s, i) => (s.tab === activeTab ? i : -1))
				.filter((i) => i >= 0);
			const absA = tabIndices[sectionIndex];
			const absB = tabIndices[sectionIndex + 1];
			if (absA === undefined || absB === undefined) return d;

			const sections = [...d.sections];
			[sections[absA], sections[absB]] = [sections[absB], sections[absA]];
			return { ...d, sections };
		});
	};

	return (
		<div className="flex flex-col h-full">
			<PageHeader
				title={
					editingName ? (
						<input
							type="text"
							className="input input-bordered input-sm w-80"
							value={nameValue}
							onChange={(e) => setNameValue(e.target.value)}
							onBlur={handleNameSubmit}
							onKeyDown={(e) => {
								if (e.key === "Enter") handleNameSubmit();
								if (e.key === "Escape") setEditingName(false);
							}}
							autoFocus
						/>
					) : (
						<span
							className="cursor-pointer hover:text-primary"
							onDoubleClick={() => {
								setNameValue(def.name);
								setEditingName(true);
							}}
							title="Double-click to rename"
						>
							{def.name}
						</span>
					)
				}
				onBack={goToPipeline}
			>
				{saving && (
					<span className="text-xs text-base-content/50">Saving...</span>
				)}
				{saved && !saving && (
					<span className="text-xs text-success">Saved</span>
				)}
				{!autosave && dirty && !saving && (
					<>
						<span className="badge badge-warning badge-sm">Unsaved</span>
						<button
							type="button"
							className="btn btn-primary btn-sm"
							onClick={() => {
								if (def) doSave(def);
							}}
						>
							Save
						</button>
					</>
				)}
			</PageHeader>

			{/* Tab bar */}
			<div className="border-b border-base-300">
				<div className="flex">
					{def.tabs.map((tab) => (
						<button
							key={tab.id}
							type="button"
							className={`flex-1 py-3 text-sm font-medium border-b-2 transition-colors ${
								activeTab === tab.id
									? "border-primary text-primary"
									: "border-transparent text-base-content/50 hover:text-base-content/80 hover:bg-base-200/50"
							}`}
							onClick={() => setActiveTab(tab.id)}
						>
							{tab.label}
						</button>
					))}
				</div>
			</div>

			{/* Sections for active tab */}
			<div className="flex-1 overflow-y-auto p-6">
				<div className="max-w-3xl mx-auto space-y-4">
					{tabSections.length === 0 && (
						<p className="text-sm text-base-content/40 text-center py-8">
							No sections in this tab yet.
						</p>
					)}
					{tabSections.map((section, idx) => (
						<SectionCard
							key={`${section.title}-${idx}`}
							section={section}
							index={idx}
							total={tabSections.length}
							onUpdate={(updater) => updateSection(idx, updater)}
							onRemove={() => removeSection(idx)}
							onMoveUp={() => moveSectionUp(idx)}
							onMoveDown={() => moveSectionDown(idx)}
						/>
					))}

					<button
						type="button"
						className="btn btn-ghost btn-sm w-full border border-dashed border-base-300"
						onClick={addSection}
					>
						+ Add Section
					</button>
				</div>
			</div>
		</div>
	);
}

// ─── Section Card ────────────────────────────────────────────────────────────

function SectionCard({
	section,
	index,
	total,
	onUpdate,
	onRemove,
	onMoveUp,
	onMoveDown,
}: {
	section: QuestionnaireSectionDef;
	index: number;
	total: number;
	onUpdate: (
		updater: (s: QuestionnaireSectionDef) => QuestionnaireSectionDef,
	) => void;
	onRemove: () => void;
	onMoveUp: () => void;
	onMoveDown: () => void;
}) {
	const [collapsed, setCollapsed] = useState(false);

	const updateQuestion = (
		qIdx: number,
		updater: (q: QuestionDef) => QuestionDef,
	) => {
		onUpdate((s) => {
			const questions = [...s.questions];
			questions[qIdx] = updater(questions[qIdx]);
			return { ...s, questions };
		});
	};

	const removeQuestion = (qIdx: number) => {
		onUpdate((s) => ({
			...s,
			questions: s.questions.filter((_, i) => i !== qIdx),
		}));
	};

	const moveQuestion = (qIdx: number, dir: -1 | 1) => {
		const target = qIdx + dir;
		onUpdate((s) => {
			if (target < 0 || target >= s.questions.length) return s;
			const questions = [...s.questions];
			[questions[qIdx], questions[target]] = [
				questions[target],
				questions[qIdx],
			];
			return { ...s, questions };
		});
	};

	const addQuestion = (kind: string) => {
		const newQ: QuestionDef =
			kind === "text"
				? { kind: "text", variable: "", label: "" }
				: kind === "conditional"
					? {
							kind: "conditional",
							variable: "",
							label: "",
						}
					: kind === "contact-list"
						? {
								kind: "contact-list",
								role: "",
								label: "",
								listVariable: "",
								property: "full_name",
							}
						: {
								kind: "contact-role",
								role: "",
								label: "",
								variableMappings: {},
							};
		onUpdate((s) => ({ ...s, questions: [...s.questions, newQ] }));
	};

	return (
		<div className="rounded-xl border border-base-300 bg-base-100">
			{/* Section header */}
			<div className="flex items-center gap-2 px-4 py-3 border-b border-base-200">
				<button
					type="button"
					className="btn btn-ghost btn-xs"
					onClick={() => setCollapsed(!collapsed)}
				>
					{collapsed ? "\u25B8" : "\u25BE"}
				</button>

				<input
					type="text"
					className="input input-ghost input-sm flex-1 font-semibold"
					value={section.title}
					onChange={(e) =>
						onUpdate((s) => ({
							...s,
							title: e.target.value,
						}))
					}
					placeholder="Section title"
				/>

				{/* Kind toggle */}
				<select
					className="select select-ghost select-xs"
					value={section.kind ?? "standard"}
					onChange={(e) =>
						onUpdate((s) => ({
							...s,
							kind: e.target.value as "standard" | "contacts",
						}))
					}
				>
					<option value="standard">Standard</option>
					<option value="contacts">Contacts</option>
				</select>

				{/* Move buttons */}
				<div className="flex gap-0.5">
					<button
						type="button"
						className="btn btn-ghost btn-xs"
						onClick={onMoveUp}
						disabled={index === 0}
						title="Move up"
					>
						&#9650;
					</button>
					<button
						type="button"
						className="btn btn-ghost btn-xs"
						onClick={onMoveDown}
						disabled={index === total - 1}
						title="Move down"
					>
						&#9660;
					</button>
				</div>

				<button
					type="button"
					className="btn btn-ghost btn-xs text-error"
					onClick={onRemove}
					title="Remove section"
				>
					&times;
				</button>
			</div>

			{/* Section body */}
			{!collapsed && (
				<div className="px-4 py-3 space-y-3">
					{/* Description */}
					<input
						type="text"
						className="input input-bordered input-sm w-full"
						value={section.description ?? ""}
						onChange={(e) =>
							onUpdate((s) => ({
								...s,
								description: e.target.value || undefined,
							}))
						}
						placeholder="Section description (optional)"
					/>

					{/* Questions */}
					{section.kind !== "contacts" && (
						<>
							{section.questions.length > 0 && (
								<SectionHeading className="mt-2">Questions</SectionHeading>
							)}
							<div className="space-y-2">
								{section.questions.map((q, qIdx) => (
									<QuestionRow
										key={qIdx}
										question={q}
										index={qIdx}
										total={section.questions.length}
										onUpdate={(updater) => updateQuestion(qIdx, updater)}
										onRemove={() => removeQuestion(qIdx)}
										onMove={(dir) => moveQuestion(qIdx, dir)}
									/>
								))}
							</div>
							{/* Add question buttons */}
							<div className="flex gap-2">
								<button
									type="button"
									className="btn btn-ghost btn-xs"
									onClick={() => addQuestion("text")}
								>
									+ Text
								</button>
								<button
									type="button"
									className="btn btn-ghost btn-xs"
									onClick={() => addQuestion("conditional")}
								>
									+ Conditional
								</button>
								<button
									type="button"
									className="btn btn-ghost btn-xs"
									onClick={() => addQuestion("contact-role")}
								>
									+ Contact Role
								</button>
								<button
									type="button"
									className="btn btn-ghost btn-xs"
									onClick={() => addQuestion("contact-list")}
								>
									+ Contact List
								</button>
							</div>
						</>
					)}

					{section.kind === "contacts" && (
						<p className="text-sm text-base-content/50 italic">
							This section renders the inline contact management list
							automatically.
						</p>
					)}
				</div>
			)}
		</div>
	);
}

// ─── Question Row ────────────────────────────────────────────────────────────

function QuestionRow({
	question,
	index,
	total,
	onUpdate,
	onRemove,
	onMove,
}: {
	question: QuestionDef;
	index: number;
	total: number;
	onUpdate: (updater: (q: QuestionDef) => QuestionDef) => void;
	onRemove: () => void;
	onMove: (dir: -1 | 1) => void;
}) {
	const q = question as Record<string, unknown>;

	return (
		<div className="flex items-start gap-2 p-2 rounded-lg bg-base-200/50 border border-base-300">
			{/* Kind badge */}
			<span className="badge badge-sm badge-outline mt-1 shrink-0">
				{question.kind}
			</span>

			{/* Fields */}
			<div className="flex-1 space-y-1">
				{question.kind === "text" && (
					<TextQuestionFields question={q} onUpdate={onUpdate} />
				)}
				{question.kind === "conditional" && (
					<ConditionalQuestionFields question={q} onUpdate={onUpdate} />
				)}
				{question.kind === "contact-role" && (
					<ContactRoleQuestionFields question={q} onUpdate={onUpdate} />
				)}
				{question.kind === "contact-list" && (
					<ContactListQuestionFields question={q} onUpdate={onUpdate} />
				)}
			</div>

			{/* Move/remove */}
			<div className="flex flex-col gap-0.5 shrink-0">
				<button
					type="button"
					className="btn btn-ghost btn-xs"
					onClick={() => onMove(-1)}
					disabled={index === 0}
				>
					&#9650;
				</button>
				<button
					type="button"
					className="btn btn-ghost btn-xs"
					onClick={() => onMove(1)}
					disabled={index === total - 1}
				>
					&#9660;
				</button>
				<button
					type="button"
					className="btn btn-ghost btn-xs text-error"
					onClick={onRemove}
				>
					&times;
				</button>
			</div>
		</div>
	);
}

// ─── Question type-specific fields ──────────────────────────────────────────

function TextQuestionFields({
	question,
	onUpdate,
}: {
	question: Record<string, unknown>;
	onUpdate: (updater: (q: QuestionDef) => QuestionDef) => void;
}) {
	return (
		<div className="flex flex-wrap gap-2">
			<input
				type="text"
				className="input input-bordered input-xs flex-1 min-w-40"
				value={(question.variable as string) ?? ""}
				onChange={(e) => onUpdate((q) => ({ ...q, variable: e.target.value }))}
				placeholder="Variable name"
			/>
			<input
				type="text"
				className="input input-bordered input-xs flex-1 min-w-40"
				value={(question.label as string) ?? ""}
				onChange={(e) => onUpdate((q) => ({ ...q, label: e.target.value }))}
				placeholder="Label"
			/>
			<input
				type="text"
				className="input input-bordered input-xs w-40"
				value={(question.placeholder as string) ?? ""}
				onChange={(e) =>
					onUpdate((q) => ({
						...q,
						placeholder: e.target.value || undefined,
					}))
				}
				placeholder="Placeholder (optional)"
			/>
			<label className="flex items-center gap-1 text-xs">
				<input
					type="checkbox"
					className="checkbox checkbox-xs"
					checked={!!question.half}
					onChange={(e) =>
						onUpdate((q) => ({
							...q,
							half: e.target.checked || undefined,
						}))
					}
				/>
				Half
			</label>
			<label className="flex items-center gap-1 text-xs">
				<input
					type="checkbox"
					className="checkbox checkbox-xs"
					checked={!!question.third}
					onChange={(e) =>
						onUpdate((q) => ({
							...q,
							third: e.target.checked || undefined,
						}))
					}
				/>
				Third
			</label>
		</div>
	);
}

function ConditionalQuestionFields({
	question,
	onUpdate,
}: {
	question: Record<string, unknown>;
	onUpdate: (updater: (q: QuestionDef) => QuestionDef) => void;
}) {
	return (
		<div className="flex flex-wrap gap-2">
			<input
				type="text"
				className="input input-bordered input-xs flex-1 min-w-40"
				value={(question.variable as string) ?? ""}
				onChange={(e) => onUpdate((q) => ({ ...q, variable: e.target.value }))}
				placeholder="Variable name"
			/>
			<input
				type="text"
				className="input input-bordered input-xs flex-1 min-w-40"
				value={(question.label as string) ?? ""}
				onChange={(e) => onUpdate((q) => ({ ...q, label: e.target.value }))}
				placeholder="Label"
			/>
			<input
				type="text"
				className="input input-bordered input-xs w-32"
				value={(question.trueLabel as string) ?? ""}
				onChange={(e) =>
					onUpdate((q) => ({
						...q,
						trueLabel: e.target.value || undefined,
					}))
				}
				placeholder="True label"
			/>
			<input
				type="text"
				className="input input-bordered input-xs w-32"
				value={(question.falseLabel as string) ?? ""}
				onChange={(e) =>
					onUpdate((q) => ({
						...q,
						falseLabel: e.target.value || undefined,
					}))
				}
				placeholder="False label"
			/>
		</div>
	);
}

function ContactListQuestionFields({
	question,
	onUpdate,
}: {
	question: Record<string, unknown>;
	onUpdate: (updater: (q: QuestionDef) => QuestionDef) => void;
}) {
	return (
		<div className="space-y-2">
			<div className="flex flex-wrap gap-2">
				<input
					type="text"
					className="input input-bordered input-xs flex-1 min-w-40"
					value={(question.role as string) ?? ""}
					onChange={(e) => onUpdate((q) => ({ ...q, role: e.target.value }))}
					placeholder="Role name"
				/>
				<input
					type="text"
					className="input input-bordered input-xs flex-1 min-w-40"
					value={(question.label as string) ?? ""}
					onChange={(e) => onUpdate((q) => ({ ...q, label: e.target.value }))}
					placeholder="Label"
				/>
			</div>
			<div className="flex flex-wrap gap-2">
				<input
					type="text"
					className="input input-bordered input-xs flex-1 min-w-40"
					value={(question.listVariable as string) ?? ""}
					onChange={(e) =>
						onUpdate((q) => ({ ...q, listVariable: e.target.value }))
					}
					placeholder="List variable (aggregated)"
				/>
				<input
					type="text"
					className="input input-bordered input-xs w-40"
					value={(question.property as string) ?? ""}
					onChange={(e) =>
						onUpdate((q) => ({
							...q,
							property: e.target.value || undefined,
						}))
					}
					placeholder="Property (e.g. full_name)"
				/>
			</div>
		</div>
	);
}

function ContactRoleQuestionFields({
	question,
	onUpdate,
}: {
	question: Record<string, unknown>;
	onUpdate: (updater: (q: QuestionDef) => QuestionDef) => void;
}) {
	const mappings = (question.variableMappings ?? {}) as Record<string, string>;
	const entries = Object.entries(mappings);

	const addMapping = () => {
		onUpdate((q) => ({
			...q,
			variableMappings: { ...mappings, "": "full_name" },
		}));
	};

	const updateMappingKey = (oldKey: string, newKey: string) => {
		const updated: Record<string, string> = {};
		for (const [k, v] of Object.entries(mappings)) {
			updated[k === oldKey ? newKey : k] = v;
		}
		onUpdate((q) => ({ ...q, variableMappings: updated }));
	};

	const updateMappingValue = (key: string, newValue: string) => {
		onUpdate((q) => ({
			...q,
			variableMappings: { ...mappings, [key]: newValue },
		}));
	};

	const removeMapping = (key: string) => {
		const { [key]: _, ...rest } = mappings;
		onUpdate((q) => ({ ...q, variableMappings: rest }));
	};

	return (
		<div className="space-y-2">
			<div className="flex gap-2">
				<input
					type="text"
					className="input input-bordered input-xs flex-1 min-w-40"
					value={(question.role as string) ?? ""}
					onChange={(e) => onUpdate((q) => ({ ...q, role: e.target.value }))}
					placeholder="Role name"
				/>
				<input
					type="text"
					className="input input-bordered input-xs flex-1 min-w-40"
					value={(question.label as string) ?? ""}
					onChange={(e) => onUpdate((q) => ({ ...q, label: e.target.value }))}
					placeholder="Label"
				/>
			</div>

			<label className="label cursor-pointer justify-start gap-2 py-0">
				<input
					type="checkbox"
					className="checkbox checkbox-xs"
					checked={Boolean(question.multiple)}
					onChange={(e) =>
						onUpdate((q) => ({ ...q, multiple: e.target.checked }))
					}
				/>
				<span className="label-text text-xs text-base-content/60">
					Ordered assignments
				</span>
			</label>

			{/* Variable mappings */}
			{entries.length > 0 && (
				<div className="pl-4 space-y-1">
					<span className="text-xs text-base-content/50">
						Variable Mappings:
					</span>
					{entries.map(([varName, prop], i) => (
						<div key={i} className="flex items-center gap-1">
							<input
								type="text"
								className="input input-bordered input-xs flex-1"
								value={varName}
								onChange={(e) => updateMappingKey(varName, e.target.value)}
								placeholder="Variable name"
							/>
							<span className="text-xs text-base-content/40">&rarr;</span>
							<select
								className="select select-bordered select-xs"
								value={prop}
								onChange={(e) => updateMappingValue(varName, e.target.value)}
							>
								<option value="full_name">full_name</option>
								<option value="first_name">first_name</option>
								<option value="last_name">last_name</option>
								<option value="relationship">relationship</option>
								<option value="phone">phone</option>
								<option value="email">email</option>
								<option value="address">address</option>
								<option value="city">city</option>
								<option value="state">state</option>
								<option value="zip">zip</option>
							</select>
							<button
								type="button"
								className="btn btn-ghost btn-xs text-error"
								onClick={() => removeMapping(varName)}
							>
								&times;
							</button>
						</div>
					))}
				</div>
			)}
			<button
				type="button"
				className="btn btn-ghost btn-xs"
				onClick={addMapping}
			>
				+ Add Mapping
			</button>
		</div>
	);
}
