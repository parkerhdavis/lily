// SPDX-License-Identifier: AGPL-3.0-or-later
import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import AssignmentPicker from "@/components/AssignmentPicker";
import ContactListPicker from "@/components/ContactListPicker";
import ContactPicker from "@/components/ContactPicker";
import QuestionnaireChooser, {
	type QuestionnaireChoice,
} from "@/components/QuestionnaireChooser";
import PageHeader from "@/components/ui/PageHeader";
import StatusDot from "@/components/ui/StatusDot";
import {
	questionnaireDef as fallbackDef,
	questionnaireTabs as fallbackTabs,
} from "@/data/questionnaireDef";
import { useWorkflowStore } from "@/stores/workflowStore";
import { RELATIONSHIP_OPTIONS } from "@/types";
import type {
	QuestionDef,
	QuestionnaireDefFile,
	QuestionnaireSectionDef,
} from "@/types/questionnaire";
import { extractFolderName } from "@/utils/path";

/** Status chip shown on every section row in the rail.
 *  Three visual states: empty (muted), partial (warning), complete (success). */
function SectionStatusChip({
	stats,
}: {
	stats: { total: number; filled: number; countText: string };
}) {
	if (!stats.countText) return null;
	const state =
		stats.total > 0 && stats.filled === stats.total
			? "complete"
			: stats.filled > 0
				? "partial"
				: "empty";
	const tone =
		state === "complete"
			? "bg-success/15 text-success"
			: state === "partial"
				? "bg-warning/15 text-warning"
				: "bg-base-300/60 text-base-content/50";
	return (
		<span
			className={`text-[10px] shrink-0 tabular-nums px-1.5 py-0.5 rounded-full font-medium ${tone}`}
		>
			{stats.countText}
		</span>
	);
}

/** Highlight search matches within text. */
function HighlightText({ text, query }: { text: string; query: string }) {
	const q = query.trim().toLowerCase();
	if (!q) return <>{text}</>;

	const idx = text.toLowerCase().indexOf(q);
	if (idx === -1) return <>{text}</>;

	return (
		<>
			{text.slice(0, idx)}
			<mark className="bg-warning/30 text-inherit rounded px-0.5">
				{text.slice(idx, idx + q.length)}
			</mark>
			{text.slice(idx + q.length)}
		</>
	);
}

export default function Questionnaire() {
	const {
		workingDir,
		lilyFile,
		saveClientVariable,
		saveQuestionnaireNote,
		addContact,
		updateContact,
		deleteContact,
		returnToHub,
		reloadLilyFile,
	} = useWorkflowStore();

	const variables = lilyFile?.variables ?? {};
	const contacts = lilyFile?.contacts ?? [];
	const notes = lilyFile?.questionnaire_notes ?? {};

	// Dynamic questionnaire definition
	const [questionnaireDef, setQuestionnaireDef] =
		useState<QuestionnaireSectionDef[]>(fallbackDef);
	const [questionnaireTabs, setQuestionnaireTabs] = useState(fallbackTabs);
	// When the client has no questionnaire chosen yet, prompt the user to pick
	// one (then remember it on the .lily file). New clients are stamped at
	// creation, so this is mainly for clients that predate point-of-use choice.
	const [needsQuestionnaire, setNeedsQuestionnaire] = useState(false);
	const [pickedQuestionnaire, setPickedQuestionnaire] =
		useState<QuestionnaireChoice | null>(null);
	const [choosing, setChoosing] = useState(false);

	const applyDef = useCallback((def: QuestionnaireDefFile) => {
		setQuestionnaireDef(def.sections);
		setQuestionnaireTabs(
			def.tabs.map((t) => ({
				id: t.id as (typeof fallbackTabs)[number]["id"],
				label: t.label,
			})),
		);
	}, []);

	// Load the client's chosen questionnaire on mount, or prompt to pick one.
	useEffect(() => {
		(async () => {
			if (!lilyFile?.questionnaire_id) {
				setNeedsQuestionnaire(true);
				return;
			}
			try {
				const def = await invoke<QuestionnaireDefFile>("load_questionnaire", {
					id: lilyFile.questionnaire_id,
				});
				applyDef(def);
				// Auto-advance the stamped version if the questionnaire moved forward.
				if (workingDir && lilyFile.questionnaire_version !== def.version) {
					invoke("set_client_questionnaire", {
						workingDir,
						questionnaireId: def.id,
						questionnaireVersion: def.version,
					}).catch((err: unknown) =>
						console.error("Failed to stamp questionnaire version:", err),
					);
				}
			} catch (err) {
				console.error("Failed to load questionnaire definition:", err);
				// The stamped questionnaire is gone — let the user pick a new one.
				setNeedsQuestionnaire(true);
			}
		})();
	}, []); // eslint-disable-line react-hooks/exhaustive-deps

	// Apply the user's questionnaire choice for an un-stamped client.
	const handleChooseQuestionnaire = useCallback(async () => {
		if (!pickedQuestionnaire || !workingDir) return;
		setChoosing(true);
		try {
			const def = await invoke<QuestionnaireDefFile>("load_questionnaire", {
				id: pickedQuestionnaire.id,
			});
			await invoke("set_client_questionnaire", {
				workingDir,
				questionnaireId: def.id,
				questionnaireVersion: def.version,
			});
			await reloadLilyFile();
			applyDef(def);
			setNeedsQuestionnaire(false);
		} catch (err) {
			console.error("Failed to set questionnaire:", err);
		} finally {
			setChoosing(false);
		}
	}, [pickedQuestionnaire, workingDir, reloadLilyFile, applyDef]);

	// Master/detail: track the single active section across all tabs
	const [activeSectionTitle, setActiveSectionTitle] = useState<string | null>(
		null,
	);

	// Save-state indicator
	const [saveStatus, setSaveStatus] = useState<"idle" | "saving" | "saved">(
		"idle",
	);
	const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

	const showSaved = useCallback(() => {
		setSaveStatus("saved");
		if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
		saveTimerRef.current = setTimeout(() => setSaveStatus("idle"), 2000);
	}, []);

	// Collect derived variable definitions from the questionnaire
	const derivedDefs = useMemo(() => {
		const defs: {
			variable: string;
			sources: string[];
			join: string;
		}[] = [];
		for (const section of questionnaireDef) {
			for (const q of section.questions) {
				if (q.kind === "derived") {
					defs.push({
						variable: q.variable,
						sources: q.sources,
						join: q.join ?? " ",
					});
				}
			}
		}
		return defs;
	}, [questionnaireDef]);

	// Build a reverse map: source variable → derived defs that depend on it
	const sourceToDeriveds = useMemo(() => {
		const map = new Map<
			string,
			{ variable: string; sources: string[]; join: string }[]
		>();
		for (const def of derivedDefs) {
			for (const src of def.sources) {
				const existing = map.get(src) ?? [];
				existing.push(def);
				map.set(src, existing);
			}
		}
		return map;
	}, [derivedDefs]);

	const handleSaveVariable = useCallback(
		async (name: string, value: string) => {
			setSaveStatus("saving");
			await saveClientVariable(name, value);

			// Recompute any derived variables whose sources include this variable
			const affected = sourceToDeriveds.get(name);
			if (affected) {
				const current = { ...variables, [name]: value };
				for (const def of affected) {
					const derived = def.sources
						.map((s) => current[s]?.trim())
						.filter(Boolean)
						.join(def.join);
					await saveClientVariable(def.variable, derived);
				}
			}

			showSaved();
		},
		[saveClientVariable, showSaved, variables, sourceToDeriveds],
	);

	const handleSaveNote = useCallback(
		async (section: string, noteKind: "client" | "internal", value: string) => {
			setSaveStatus("saving");
			await saveQuestionnaireNote(section, noteKind, value);
			showSaved();
		},
		[saveQuestionnaireNote, showSaved],
	);

	// Wrap contact ops to trigger save indicator
	const handleAddContact = useCallback(
		async (contact: Omit<import("@/types").Contact, "id">) => {
			setSaveStatus("saving");
			const result = await addContact(contact);
			showSaved();
			return result;
		},
		[addContact, showSaved],
	);

	const handleUpdateContact = useCallback(
		async (contact: import("@/types").Contact) => {
			setSaveStatus("saving");
			await updateContact(contact);
			showSaved();
		},
		[updateContact, showSaved],
	);

	const handleDeleteContact = useCallback(
		async (contactId: string) => {
			setSaveStatus("saving");
			await deleteContact(contactId);
			showSaved();
		},
		[deleteContact, showSaved],
	);

	useEffect(() => {
		return () => {
			if (saveTimerRef.current) clearTimeout(saveTimerRef.current);
		};
	}, []);

	// Search
	const [search, setSearch] = useState("");
	const searchRef = useRef<HTMLInputElement>(null);

	// Ctrl+F to focus search
	useEffect(() => {
		const handler = (e: KeyboardEvent) => {
			if ((e.ctrlKey || e.metaKey) && e.key === "f") {
				e.preventDefault();
				searchRef.current?.focus();
			}
		};
		document.addEventListener("keydown", handler);
		return () => document.removeEventListener("keydown", handler);
	}, []);

	// Filter all sections by search query (across every tab)
	const filteredSections = useMemo(() => {
		const q = search.trim().toLowerCase();
		if (!q) return questionnaireDef;

		const tokens = q.split(/\s+/);
		return questionnaireDef.filter((section) => {
			if (tokens.every((t) => section.title.toLowerCase().includes(t)))
				return true;
			return section.questions.some((question) => {
				const label =
					question.kind === "contact-role"
						? question.label
						: question.kind === "text"
							? `${question.label} ${question.variable}`
							: question.label;
				return tokens.every((t) => label.toLowerCase().includes(t));
			});
		});
	}, [questionnaireDef, search]);

	// Group filtered sections by tab for rail rendering, preserving tab order
	const railGroups = useMemo(() => {
		return questionnaireTabs
			.map((tab) => ({
				tab,
				sections: filteredSections.filter((s) => s.tab === tab.id),
			}))
			.filter((g) => g.sections.length > 0);
	}, [filteredSections, questionnaireTabs]);

	// Default-select the first section overall once defs are loaded
	useEffect(() => {
		if (activeSectionTitle) return;
		const first = questionnaireDef[0];
		if (first) setActiveSectionTitle(first.title);
	}, [questionnaireDef, activeSectionTitle]);

	// If search filters out the currently active section, jump to the first match
	useEffect(() => {
		if (!search.trim() || filteredSections.length === 0) return;
		if (
			activeSectionTitle &&
			filteredSections.some((s) => s.title === activeSectionTitle)
		)
			return;
		setActiveSectionTitle(filteredSections[0].title);
	}, [filteredSections, search, activeSectionTitle]);

	const activeSection =
		questionnaireDef.find((s) => s.title === activeSectionTitle) ??
		filteredSections[0] ??
		null;
	const activeIdx = activeSection
		? filteredSections.findIndex((s) => s.title === activeSection.title)
		: -1;
	const nextSection =
		activeIdx >= 0 && activeIdx < filteredSections.length - 1
			? filteredSections[activeIdx + 1]
			: null;
	const prevSection = activeIdx > 0 ? filteredSections[activeIdx - 1] : null;

	const selectSection = (title: string) => {
		setActiveSectionTitle(title);
	};

	// Completion stats
	const stats = useMemo(() => {
		let total = 0;
		let filled = 0;
		for (const section of questionnaireDef) {
			if (section.kind === "contacts") {
				total++;
				if (contacts.length > 0) filled++;
				continue;
			}
			for (const q of section.questions) {
				if (q.kind === "text") {
					total++;
					if (variables[q.variable]?.trim()) filled++;
				}
			}
		}
		return { total, filled };
	}, [variables, contacts, questionnaireDef]);

	const bindings = lilyFile?.contact_bindings ?? {};

	// Per-section stats. Counts every non-derived question kind so every
	// section gets a meaningful filled/total. The "contacts" section kind has
	// no fixed total — it reports the contact count directly.
	const sectionStats = useMemo(() => {
		const map: Record<
			string,
			{ total: number; filled: number; countText: string }
		> = {};
		for (const section of questionnaireDef) {
			if (section.kind === "contacts") {
				const n = contacts.length;
				map[section.title] = {
					total: 1,
					filled: n > 0 ? 1 : 0,
					countText: String(n),
				};
				continue;
			}
			let total = 0;
			let filled = 0;
			for (const q of section.questions) {
				if (q.kind === "text") {
					total++;
					if (variables[q.variable]?.trim()) filled++;
				} else if (q.kind === "conditional") {
					total++;
					const v = variables[q.variable];
					if (v === "true" || v === "false") filled++;
				} else if (q.kind === "contact-role") {
					total++;
					if (bindings[q.role]) filled++;
				} else if (q.kind === "contact-list") {
					total++;
					if ((bindings[q.role]?.contact_ids?.length ?? 0) > 0) filled++;
				}
			}
			map[section.title] = {
				total,
				filled,
				countText: total > 0 ? `${filled}/${total}` : "",
			};
		}
		return map;
	}, [variables, contacts, bindings, questionnaireDef]);

	const folderName = workingDir ? extractFolderName(workingDir) : "Client";

	const isContacts = activeSection?.kind === "contacts";

	// No questionnaire chosen for this client yet \u2014 prompt the user to pick one.
	if (needsQuestionnaire) {
		return (
			<div className="flex flex-col h-full">
				<PageHeader
					title={`${folderName} \u2014 Questionnaire`}
					onBack={returnToHub}
				/>
				<div className="flex-1 flex flex-col items-center justify-center gap-4 p-8">
					<h3 className="text-lg font-semibold">Choose a questionnaire</h3>
					<p className="text-sm text-base-content/60 text-center max-w-sm">
						Select which questionnaire to use for {folderName}. It's saved to
						this client and used whenever you open their questionnaire.
					</p>
					<div className="w-full max-w-xs rounded-lg border border-base-300 bg-base-200/40 p-3">
						<QuestionnaireChooser onChange={setPickedQuestionnaire} />
					</div>
					<button
						type="button"
						className="btn btn-primary btn-sm"
						onClick={handleChooseQuestionnaire}
						disabled={!pickedQuestionnaire || choosing}
					>
						{choosing ? (
							<span className="loading loading-spinner loading-xs" />
						) : null}
						Use this questionnaire
					</button>
				</div>
			</div>
		);
	}

	return (
		<div className="flex flex-col h-full">
			<PageHeader
				title={`${folderName} \u2014 Questionnaire`}
				onBack={returnToHub}
			>
				<span className="text-sm text-base-content/60 flex items-center gap-2 whitespace-nowrap">
					{saveStatus === "saving" && (
						<span className="text-warning flex items-center gap-1 text-xs">
							<span className="loading loading-spinner loading-xs" />
							Saving...
						</span>
					)}
					{saveStatus === "saved" && (
						<span className="text-success text-xs">All changes saved</span>
					)}
					{stats.filled} / {stats.total} fields filled
				</span>
			</PageHeader>

			{/* Master/detail body */}
			<div className="flex flex-1 min-h-0">
				{/* Section rail */}
				<aside className="w-72 shrink-0 border-r border-base-300 bg-base-200/50 flex flex-col">
					<div className="p-3 border-b border-base-300">
						<input
							ref={searchRef}
							type="text"
							className="input input-bordered input-xs w-full"
							placeholder="Search fields... (Ctrl+F)"
							value={search}
							onChange={(e) => setSearch(e.target.value)}
						/>
					</div>
					<nav className="flex-1 overflow-y-auto p-2">
						{railGroups.length === 0 ? (
							<p className="text-xs text-base-content/50 text-center py-6 px-2">
								{search ? "No matching fields." : "No sections."}
							</p>
						) : (
							<div className="flex flex-col gap-4">
								{railGroups.map(({ tab, sections }) => (
									<div key={tab.id}>
										<div className="px-3 pt-1 pb-1.5 text-[10px] font-semibold uppercase tracking-wider text-base-content/40">
											{tab.label}
										</div>
										<ul className="flex flex-col gap-1">
											{sections.map((section) => {
												const ss = sectionStats[section.title];
												const isActive = section.title === activeSection?.title;
												return (
													<li key={section.title}>
														<button
															type="button"
															onClick={() => selectSection(section.title)}
															className={`w-full text-left px-3 py-2 rounded-lg transition-colors ${
																isActive
																	? "bg-primary/15 text-primary border border-primary/30"
																	: "border border-transparent hover:bg-base-300/60"
															}`}
														>
															<div className="flex items-center gap-2">
																<span className="flex-1 min-w-0 text-sm font-medium truncate">
																	<HighlightText
																		text={section.title}
																		query={search}
																	/>
																</span>
																{ss && <SectionStatusChip stats={ss} />}
															</div>
														</button>
													</li>
												);
											})}
										</ul>
									</div>
								))}
							</div>
						)}
					</nav>
				</aside>

				{/* Section pane */}
				<main className="flex-1 overflow-y-auto">
					{activeSection ? (
						<div className="max-w-3xl mx-auto px-8 py-6 pb-32">
							<div className="mb-6">
								<h2 className="text-2xl font-semibold">
									{activeSection.title}
								</h2>
								{activeSection.description && (
									<p className="text-sm text-base-content/60 mt-1">
										{activeSection.description}
									</p>
								)}
							</div>

							{isContacts ? (
								<InlineContactList
									contacts={contacts}
									onAdd={handleAddContact}
									onUpdate={handleUpdateContact}
									onDelete={handleDeleteContact}
								/>
							) : (
								<div className="grid grid-cols-6 gap-x-3 gap-y-4">
									{activeSection.questions.map((q) => {
										const span =
											q.kind === "text" && q.third
												? "col-span-2"
												: q.kind === "text" && q.half
													? "col-span-3"
													: "col-span-6";
										return (
											<div
												key={
													q.kind === "contact-role" || q.kind === "contact-list"
														? q.role
														: q.variable
												}
												className={span}
											>
												<QuestionField
													question={q}
													value={
														q.kind === "contact-role" ||
														q.kind === "contact-list"
															? ""
															: (variables[q.variable] ?? "")
													}
													onSave={handleSaveVariable}
													searchQuery={search}
												/>
											</div>
										);
									})}
								</div>
							)}

							<SectionNotesFields
								sectionTitle={activeSection.title}
								clientNotes={notes[activeSection.title]?.client ?? ""}
								internalNotes={notes[activeSection.title]?.internal ?? ""}
								onSave={handleSaveNote}
							/>

							<div className="mt-10 pt-4 border-t border-base-300 flex items-center justify-between gap-3">
								<button
									type="button"
									className="btn btn-ghost btn-sm"
									onClick={() =>
										prevSection && selectSection(prevSection.title)
									}
									disabled={!prevSection}
								>
									&#9664; {prevSection?.title ?? "Previous"}
								</button>
								<button
									type="button"
									className="btn btn-primary btn-sm"
									onClick={() =>
										nextSection && selectSection(nextSection.title)
									}
									disabled={!nextSection}
								>
									{nextSection?.title ?? "Done"} &#9654;
								</button>
							</div>
						</div>
					) : (
						<div className="h-full flex items-center justify-center">
							<p className="text-sm text-base-content/50">
								{search
									? "No fields match your search."
									: "No sections defined."}
							</p>
						</div>
					)}
				</main>
			</div>
		</div>
	);
}

// ─── Inline contact list ────────────────────────────────────────────────────

function InlineContactList({
	contacts,
	onAdd,
	onUpdate,
	onDelete,
}: {
	contacts: {
		id: string;
		full_name: string;
		relationship: string;
		other_relationship?: string;
		phone: string;
		email: string;
	}[];
	onAdd: (
		contact: Omit<import("@/types").Contact, "id">,
	) => Promise<import("@/types").Contact>;
	onUpdate: (contact: import("@/types").Contact) => Promise<void>;
	onDelete: (contactId: string) => Promise<void>;
}) {
	const [editingId, setEditingId] = useState<string | null>(null);

	const handleAdd = async () => {
		// Immediately create an empty contact and open it for editing
		const created = await onAdd({
			full_name: "",
			first_name: "",
			middle_name: "",
			last_name: "",
			relationship: "",
			other_relationship: "",
			phone: "",
			email: "",
			address: "",
			city: "",
			state: "",
			zip: "",
			is_minor: false,
		});
		setEditingId(created.id);
	};

	return (
		<div className="space-y-3">
			{contacts.length === 0 && editingId === null && (
				<p className="text-sm text-base-content/50">No contacts added yet.</p>
			)}

			{contacts.map((c) =>
				editingId === c.id ? (
					<ContactEditForm
						key={c.id}
						contactId={c.id}
						onSave={onUpdate}
						onCancel={() => setEditingId(null)}
					/>
				) : (
					<div
						key={c.id}
						className="flex items-center gap-3 p-3 rounded-lg border border-base-300 group"
					>
						<div className="flex-1 min-w-0">
							<div className="font-medium truncate">
								{c.full_name || "Unnamed"}
							</div>
							<div className="text-xs text-base-content/50 truncate">
								{[
									c.relationship === "Other" && c.other_relationship
										? c.other_relationship
										: c.relationship,
									c.phone,
									c.email,
								]
									.filter(Boolean)
									.join(" \u00B7 ") || "No details"}
							</div>
						</div>
						<button
							type="button"
							className="btn btn-ghost btn-xs opacity-0 group-hover:opacity-100"
							onClick={() => setEditingId(c.id)}
						>
							Edit
						</button>
						<button
							type="button"
							className="btn btn-ghost btn-xs text-error opacity-0 group-hover:opacity-100"
							onClick={() => onDelete(c.id)}
						>
							&times;
						</button>
					</div>
				),
			)}

			<button
				type="button"
				className="btn btn-outline btn-sm w-full"
				onClick={handleAdd}
			>
				+ Add Contact
			</button>
		</div>
	);
}

/** Inline form for editing a contact with side-by-side fields.
 *  Auto-saves each field on blur. */
function ContactEditForm({
	contactId,
	onSave,
	onCancel,
}: {
	contactId: string | null;
	onSave: (contact: import("@/types").Contact) => Promise<void>;
	onCancel: () => void;
}) {
	const lilyFile = useWorkflowStore((s) => s.lilyFile);
	const existing = contactId
		? (lilyFile?.contacts?.find((c) => c.id === contactId) ?? null)
		: null;

	const [form, setForm] = useState({
		full_name: existing?.full_name ?? "",
		first_name: existing?.first_name ?? "",
		middle_name: existing?.middle_name ?? "",
		last_name: existing?.last_name ?? "",
		relationship: existing?.relationship ?? "",
		other_relationship: existing?.other_relationship ?? "",
		phone: existing?.phone ?? "",
		email: existing?.email ?? "",
		address: existing?.address ?? "",
		city: existing?.city ?? "",
		state: existing?.state ?? "",
		zip: existing?.zip ?? "",
		is_minor: existing?.is_minor ?? false,
	});

	// Track the last-saved snapshot to avoid redundant saves
	const savedRef = useRef({ ...form });

	const update = (key: string, value: string | boolean) =>
		setForm((prev) => {
			const next = { ...prev, [key]: value };
			// Auto-construct full_name from first/middle/last
			if (
				key === "first_name" ||
				key === "middle_name" ||
				key === "last_name"
			) {
				next.full_name = [next.first_name, next.middle_name, next.last_name]
					.map((s) => (typeof s === "string" ? s.trim() : ""))
					.filter(Boolean)
					.join(" ");
			}
			return next;
		});

	const handleFieldBlur = useCallback(async () => {
		const current = form;
		const saved = savedRef.current;
		const changed = Object.keys(current).some(
			(k) =>
				current[k as keyof typeof current] !== saved[k as keyof typeof saved],
		);
		if (changed) {
			savedRef.current = { ...current };
			await onSave({ id: contactId ?? "", ...current });
		}
	}, [form, contactId, onSave]);

	const textFields: {
		key: string;
		label: string;
		span: 6 | 3 | 2;
	}[] = [
		{ key: "first_name", label: "First Name", span: 2 },
		{ key: "middle_name", label: "Middle Name", span: 2 },
		{ key: "last_name", label: "Last Name", span: 2 },
		{ key: "phone", label: "Phone", span: 3 },
		{ key: "email", label: "Email", span: 3 },
		{ key: "address", label: "Address", span: 6 },
		{ key: "city", label: "City", span: 2 },
		{ key: "state", label: "State", span: 2 },
		{ key: "zip", label: "ZIP", span: 2 },
	];

	return (
		<div className="p-3 rounded-lg border border-primary/30 bg-primary/5 space-y-3">
			{form.full_name && (
				<p className="text-sm text-base-content/60">{form.full_name}</p>
			)}
			<div className="grid grid-cols-6 gap-2">
				{/* Relationship dropdown row */}
				<div
					className={`col-span-${form.relationship === "Other" ? "3" : "6"}`}
				>
					<label className="label pb-0.5">
						<span className="label-text text-xs flex items-center gap-1">
							<StatusDot filled={Boolean(form.relationship)} />
							Relationship
						</span>
					</label>
					<select
						className="select select-bordered select-sm w-full"
						value={form.relationship}
						onChange={(e) => {
							update("relationship", e.target.value);
							if (e.target.value !== "Other") {
								update("other_relationship", "");
							}
							if (e.target.value !== "Child") {
								update("is_minor", false);
							}
						}}
						onBlur={handleFieldBlur}
					>
						<option value="">Select relationship...</option>
						{RELATIONSHIP_OPTIONS.map((opt) => (
							<option key={opt} value={opt}>
								{opt}
							</option>
						))}
					</select>
				</div>
				{form.relationship === "Other" && (
					<div className="col-span-3">
						<label className="label pb-0.5">
							<span className="label-text text-xs flex items-center gap-1">
								<StatusDot filled={Boolean(form.other_relationship.trim())} />
								Other Relationship
							</span>
						</label>
						<input
							type="text"
							className="input input-bordered input-sm w-full"
							placeholder="Specify relationship..."
							value={form.other_relationship}
							onChange={(e) => update("other_relationship", e.target.value)}
							onBlur={handleFieldBlur}
						/>
					</div>
				)}
				{form.relationship === "Child" && (
					<div className="col-span-6">
						<label className="label cursor-pointer justify-start gap-2 py-0">
							<input
								type="checkbox"
								className="checkbox checkbox-sm"
								checked={form.is_minor}
								onChange={(e) => {
									update("is_minor", e.target.checked);
									// Save immediately on toggle
									const next = { ...form, is_minor: e.target.checked };
									savedRef.current = { ...next };
									onSave({ id: contactId ?? "", ...next });
								}}
							/>
							<span className="label-text text-xs">Is Minor</span>
						</label>
					</div>
				)}
				{/* Text fields */}
				{textFields.map(({ key, label, span }) => (
					<div
						key={key}
						className={
							span === 6
								? "col-span-6"
								: span === 3
									? "col-span-3"
									: "col-span-2"
						}
					>
						<label className="label pb-0.5">
							<span className="label-text text-xs flex items-center gap-1">
								<StatusDot
									filled={Boolean(
										(form[key as keyof typeof form] as string).trim(),
									)}
								/>
								{label}
							</span>
						</label>
						<input
							type="text"
							className="input input-bordered input-sm w-full"
							value={form[key as keyof typeof form] as string}
							onChange={(e) => update(key, e.target.value)}
							onBlur={handleFieldBlur}
						/>
					</div>
				))}
			</div>
			<div className="flex justify-end">
				<button
					type="button"
					className="btn btn-ghost btn-xs"
					onClick={onCancel}
				>
					Done
				</button>
			</div>
		</div>
	);
}

// ─── Section notes (collapsible) ────────────────────────────────────────────

function SectionNotesFields({
	sectionTitle,
	clientNotes,
	internalNotes,
	onSave,
}: {
	sectionTitle: string;
	clientNotes: string;
	internalNotes: string;
	onSave: (
		section: string,
		noteKind: "client" | "internal",
		value: string,
	) => Promise<void>;
}) {
	const hasNotes = Boolean(clientNotes || internalNotes);
	const [open, setOpen] = useState(hasNotes);

	const [localClient, setLocalClient] = useState(clientNotes);
	const [localInternal, setLocalInternal] = useState(internalNotes);

	// Sync from props
	const [prevClient, setPrevClient] = useState(clientNotes);
	const [prevInternal, setPrevInternal] = useState(internalNotes);
	if (clientNotes !== prevClient) {
		setPrevClient(clientNotes);
		setLocalClient(clientNotes);
	}
	if (internalNotes !== prevInternal) {
		setPrevInternal(internalNotes);
		setLocalInternal(internalNotes);
	}

	return (
		<div className="mt-3 pt-3 border-t border-base-200">
			<button
				type="button"
				className="text-xs text-base-content/40 hover:text-base-content/60 transition-colors flex items-center gap-1"
				onClick={() => setOpen(!open)}
			>
				<span className={`transition-transform ${open ? "rotate-90" : ""}`}>
					&#9654;
				</span>
				Notes
				{hasNotes && (
					<span className="inline-block size-1.5 rounded-full bg-primary/50" />
				)}
			</button>
			{open && (
				<div className="mt-2 space-y-3">
					<div className="form-control w-full">
						<label className="label pb-1">
							<span className="label-text text-xs text-base-content/50">
								Client Notes
							</span>
						</label>
						<textarea
							className="textarea textarea-bordered textarea-sm w-full min-h-16 text-sm"
							placeholder="Notes from/for the client..."
							value={localClient}
							onChange={(e) => setLocalClient(e.target.value)}
							onBlur={() => {
								if (localClient !== clientNotes) {
									onSave(sectionTitle, "client", localClient);
								}
							}}
						/>
					</div>
					<div className="form-control w-full">
						<label className="label pb-1">
							<span className="label-text text-xs text-base-content/50">
								Internal Notes
							</span>
						</label>
						<textarea
							className="textarea textarea-bordered textarea-sm w-full min-h-16 text-sm"
							placeholder="Internal notes for the legal team..."
							value={localInternal}
							onChange={(e) => setLocalInternal(e.target.value)}
							onBlur={() => {
								if (localInternal !== internalNotes) {
									onSave(sectionTitle, "internal", localInternal);
								}
							}}
						/>
					</div>
				</div>
			)}
		</div>
	);
}

// ─── Question renderers ─────────────────────────────────────────────────────

function QuestionField({
	question,
	value,
	onSave,
	searchQuery,
}: {
	question: QuestionDef;
	value: string;
	onSave: (name: string, value: string) => Promise<void>;
	searchQuery?: string;
}) {
	switch (question.kind) {
		case "text":
			return (
				<TextQuestion
					question={question}
					value={value}
					onSave={onSave}
					searchQuery={searchQuery}
				/>
			);
		case "conditional":
			return (
				<ConditionalQuestion
					question={question}
					value={value}
					onSave={onSave}
					searchQuery={searchQuery}
				/>
			);
		case "contact-role":
			return question.multiple ? (
				<AssignmentPicker question={question} />
			) : (
				<ContactPicker question={question} />
			);
		case "contact-list":
			return <ContactListPicker question={question} />;
	}
}

function TextQuestion({
	question,
	value,
	onSave,
	searchQuery,
}: {
	question: Extract<QuestionDef, { kind: "text" }>;
	value: string;
	onSave: (name: string, value: string) => Promise<void>;
	searchQuery?: string;
}) {
	const [localValue, setLocalValue] = useState(value);

	const [prevValue, setPrevValue] = useState(value);
	if (value !== prevValue) {
		setPrevValue(value);
		setLocalValue(value);
	}

	const handleBlur = () => {
		if (localValue !== value) {
			onSave(question.variable, localValue);
		}
	};

	const handleKeyDown = (e: React.KeyboardEvent) => {
		if (e.key === "Enter") {
			(e.target as HTMLInputElement).blur();
		}
	};

	return (
		<div className="form-control w-full">
			<label className="label pb-1">
				<span className="label-text text-sm font-medium flex items-center gap-1.5">
					<StatusDot filled={Boolean(localValue.trim())} />
					<HighlightText text={question.label} query={searchQuery ?? ""} />
				</span>
			</label>
			<input
				type="text"
				className="input input-bordered input-sm w-full"
				placeholder={question.placeholder ?? `Enter ${question.label}`}
				value={localValue}
				onChange={(e) => setLocalValue(e.target.value)}
				onBlur={handleBlur}
				onKeyDown={handleKeyDown}
			/>
		</div>
	);
}

function ConditionalQuestion({
	question,
	value,
	onSave,
	searchQuery,
}: {
	question: Extract<QuestionDef, { kind: "conditional" }>;
	value: string;
	onSave: (name: string, value: string) => Promise<void>;
	searchQuery?: string;
}) {
	const isTrue = value === "true";
	const isFalse = value === "false";
	const trueLabel = question.trueLabel ?? "True";
	const falseLabel = question.falseLabel ?? "False";

	return (
		<div className="form-control w-full">
			<label className="label pb-1">
				<span className="label-text text-sm font-medium">
					<HighlightText text={question.label} query={searchQuery ?? ""} />
				</span>
			</label>
			<div className="flex rounded-lg overflow-hidden border border-base-300">
				<button
					type="button"
					className={`flex-1 text-xs font-semibold py-2 transition-colors ${
						isTrue
							? "bg-success text-success-content"
							: "bg-base-200 text-base-content/40 hover:bg-base-300"
					}`}
					onClick={() => onSave(question.variable, "true")}
				>
					{trueLabel}
				</button>
				<button
					type="button"
					className={`flex-1 text-xs font-semibold py-2 transition-colors ${
						isFalse
							? "bg-error text-error-content"
							: "bg-base-200 text-base-content/40 hover:bg-base-300"
					}`}
					onClick={() => onSave(question.variable, "false")}
				>
					{falseLabel}
				</button>
			</div>
		</div>
	);
}
