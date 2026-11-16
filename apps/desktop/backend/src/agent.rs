// SPDX-License-Identifier: AGPL-3.0-or-later
use crate::lily_file::{self, LilyFile};
use crate::plan::{self, AgentFlag, PlanCandidateReport};
use crate::provenance::{self, AgentProvenance, FlagSummary};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Merge the agent's proposed client state into the current one. Only the
/// variable values and contact bindings are taken from the proposal; contacts,
/// documents, workflow state, and the questionnaire identity stay as they are.
pub fn merge_proposal(current: &LilyFile, proposal: &LilyFile) -> LilyFile {
    let mut merged = current.clone();
    for (key, value) in &proposal.variables {
        merged.variables.insert(key.clone(), value.clone());
    }
    merged.contact_bindings = proposal.contact_bindings.clone();
    merged
}

#[derive(Debug, Deserialize)]
struct AgentFlags {
    #[serde(default)]
    flags: Vec<AgentFlag>,
}

fn read_flags(path: &Path) -> Vec<AgentFlag> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    if let Ok(parsed) = serde_json::from_str::<Vec<AgentFlag>>(&text) {
        return parsed;
    }
    serde_json::from_str::<AgentFlags>(&text)
        .map(|parsed| parsed.flags)
        .unwrap_or_default()
}

fn flag_summary(flags: &[AgentFlag]) -> FlagSummary {
    let mut summary = FlagSummary {
        total: flags.len(),
        ..FlagSummary::default()
    };
    for flag in flags {
        match flag.severity.as_str() {
            "gap" => summary.gaps += 1,
            "warning" => summary.warnings += 1,
            _ => summary.info += 1,
        }
    }
    summary
}

fn read_summary(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn expand_home(value: &str) -> PathBuf {
    if let Some(rest) = value.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(value)
}

fn default_repo_dir() -> Option<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join("Software").join("lily"))
        .filter(|path| path.is_dir())
}

fn agent_out_dir() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("lily-agent-{}-{}", std::process::id(), unique))
}

fn prompt(
    client_dir: &Path,
    plan_path: &Path,
    docx_path: &Path,
    out_dir: &Path,
    repo_dir: Option<&Path>,
) -> String {
    let repo = repo_dir
        .map(|path| format!("The Lily repository and CLI are at {}.\n", path.display()))
        .unwrap_or_default();
    format!(
        r#"You are Lily's drafting agent, working in assist mode.

Client directory: {client}
Plan: {plan}
Document: {docx}
Output directory: {out}
{repo}
Use the lily-draft skill. Audit the plan against the client's .lily file and propose only changes the client's own answers support. Do not invent legal text; when the plan cannot express what the client needs, record a gap.

Write exactly these files in the output directory:

- `proposal.lily`: a copy of the client's .lily file with only the `variables` and `contact_bindings` fields changed. Keep every other field as it is.
- `flags.json`: a JSON array of objects with `severity` (`info`, `warning`, or `gap`), `message`, and an optional `path`.
- `summary.md`: a short explanation of every proposed change and every flag.

Do not edit the client's .lily file, the plan, or the document. Do not edit OOXML. Finish when those three files are written.
"#,
        client = client_dir.display(),
        plan = plan_path.display(),
        docx = docx_path.display(),
        out = out_dir.display(),
        repo = repo,
    )
}

struct AgentRun<'a> {
    command: &'a str,
    out_dir: &'a Path,
    repo_dir: Option<&'a Path>,
    work_dir: &'a Path,
    client_dir: &'a Path,
    plan_path: &'a Path,
    docx_path: &'a Path,
    prompt: &'a str,
    model: Option<&'a str>,
}

fn run_command(run: AgentRun<'_>) -> Result<(), String> {
    let mut process = Command::new(run.command);
    process
        .arg("run")
        .arg("--format")
        .arg("json")
        .arg("--auto")
        .arg("--title")
        .arg("Lily draft agent");
    if let Some(model) = run.model.filter(|value| !value.trim().is_empty()) {
        process.arg("--model").arg(model);
    }
    process.arg(run.prompt);
    process
        .current_dir(run.work_dir)
        .env("LILY_AGENT_OUT", run.out_dir)
        .env("LILY_AGENT_REPO", run.repo_dir.unwrap_or(run.work_dir))
        .env("LILY_AGENT_CLIENT", run.client_dir)
        .env("LILY_AGENT_PLAN", run.plan_path)
        .env("LILY_AGENT_DOCX", run.docx_path);
    let output = process.output().map_err(|error| {
        format!(
            "Failed to start the drafting agent '{}': {}",
            run.command, error
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(format!(
            "The drafting agent exited with {}: {}{}",
            output.status,
            stderr.trim(),
            if stdout.trim().is_empty() {
                String::new()
            } else {
                format!("\n{}", stdout.trim())
            }
        ));
    }
    Ok(())
}

/// Run the drafting agent against a plan and client, then render its proposal
/// as a candidate document for the attorney's review.
#[tauri::command]
pub fn run_draft_agent(
    docx_path: String,
    plan_path: String,
    skeleton_path: String,
    agent_command: Option<String>,
    agent_model: Option<String>,
    agent_repo_dir: Option<String>,
) -> Result<PlanCandidateReport, String> {
    let docx = Path::new(&docx_path);
    if !docx.is_file() {
        return Err(format!("Document not found: {}", docx.display()));
    }
    let plan_file = Path::new(&plan_path);
    if !plan_file.is_file() {
        return Err(format!("Plan not found: {}", plan_file.display()));
    }
    let client_dir = docx
        .parent()
        .ok_or_else(|| "Document has no parent directory".to_string())?;
    let out_dir = agent_out_dir();
    fs::create_dir_all(&out_dir)
        .map_err(|error| format!("Failed to create agent output directory: {}", error))?;

    let repo_dir = agent_repo_dir
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(expand_home)
        .or_else(default_repo_dir);
    let work_dir = repo_dir
        .as_deref()
        .filter(|path| path.is_dir())
        .unwrap_or(client_dir);
    let command = agent_command
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("opencode");
    let prompt = prompt(
        client_dir,
        plan_file,
        docx,
        &out_dir,
        repo_dir.as_deref(),
    );

    if let Err(error) = run_command(AgentRun {
        command,
        out_dir: &out_dir,
        repo_dir: repo_dir.as_deref(),
        work_dir,
        client_dir,
        plan_path: plan_file,
        docx_path: docx,
        prompt: &prompt,
        model: agent_model.as_deref(),
    }) {
        let _ = fs::remove_dir_all(&out_dir);
        return Err(error);
    }

    let proposal_path = out_dir.join("proposal.lily");
    let proposal_text = fs::read_to_string(&proposal_path).map_err(|error| {
        format!(
            "The drafting agent did not write {}: {}",
            proposal_path.display(),
            error
        )
    })?;
    if let Err(error) = serde_json::from_str::<LilyFile>(&proposal_text) {
        let _ = fs::remove_dir_all(&out_dir);
        return Err(format!(
            "The drafting agent wrote an invalid proposal.lily: {}",
            error
        ));
    }

    let temp_client = out_dir.join("client");
    fs::create_dir_all(&temp_client)
        .map_err(|error| format!("Failed to create proposal client directory: {}", error))?;
    fs::write(temp_client.join("Client.lily"), proposal_text)
        .map_err(|error| format!("Failed to write proposal client file: {}", error))?;

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let candidate = out_dir.join(format!("lily-candidate-agent-{}.docx", unique));
    let mut report = plan::render_plan(
        plan_file,
        &temp_client,
        Path::new(&skeleton_path),
        &candidate,
        None,
    )
    .inspect_err(|_| {
        let _ = fs::remove_dir_all(&out_dir);
    })?;
    let flags = read_flags(&out_dir.join("flags.json"));
    report.provenance.agent = Some(AgentProvenance {
        command: command.to_string(),
        model: agent_model.clone(),
        prompt_hash: provenance::sha256_hex(prompt.as_bytes()),
        proposal_path: proposal_path.to_string_lossy().to_string(),
        flags: flag_summary(&flags),
    });
    provenance::write_candidate_provenance(&candidate, &report.provenance).inspect_err(|_| {
        let _ = fs::remove_dir_all(&out_dir);
    })?;
    let candidate_html =
        crate::docx_ops::get_document_html(candidate.to_string_lossy().to_string()).inspect_err(
            |_| {
                let _ = fs::remove_dir_all(&out_dir);
            },
        )?;
    let diff = crate::history::diff_document_pair_paths(docx, &candidate).inspect_err(|_| {
        let _ = fs::remove_dir_all(&out_dir);
    })?;

    Ok(PlanCandidateReport {
        candidate_path: candidate.to_string_lossy().to_string(),
        candidate_html,
        diff,
        plan_id: report.plan_id,
        plan_version: report.plan_version,
        blocks_rendered: report.blocks_rendered,
        warnings: report.warnings,
        flags,
        summary: read_summary(&out_dir.join("summary.md")),
        proposal_path: proposal_path.to_string_lossy().to_string(),
        provenance: report.provenance,
    })
}

/// Merge an approved agent proposal into the client, then replace the document
/// with the reviewed candidate. The client's .lily file is backed up first.
#[tauri::command]
pub fn approve_agent_candidate(
    docx_path: String,
    candidate_path: String,
    client_dir: String,
    proposal_path: String,
    approved_by: Option<String>,
) -> Result<(), String> {
    let candidate = Path::new(&candidate_path);
    let name = candidate
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if !name.starts_with("lily-candidate-") {
        return Err(format!("Not a Lily candidate: {}", candidate.display()));
    }
    if !candidate.is_file() {
        return Err(format!("Candidate not found: {}", candidate.display()));
    }
    let proposal_text = fs::read_to_string(&proposal_path)
        .map_err(|error| format!("Failed to read proposal {}: {}", proposal_path, error))?;
    let proposal: LilyFile = serde_json::from_str(&proposal_text)
        .map_err(|error| format!("Failed to parse proposal: {}", error))?;
    let current = lily_file::read_lily_file(&client_dir)?;
    let merged = merge_proposal(&current, &proposal);
    lily_file::backup_client_lily(&client_dir)?;
    lily_file::write_client_lily(&client_dir, &merged)?;
    let candidate_parent = candidate.parent().map(|path| path.to_path_buf());
    plan::approve_plan_candidate(docx_path, candidate_path, approved_by)?;
    if let Some(parent) = candidate_parent {
        if parent
            .file_name()
            .and_then(|value| value.to_str())
            .map(|value| value.starts_with("lily-agent-"))
            .unwrap_or(false)
        {
            let _ = fs::remove_dir_all(parent);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(name: &str, variables: &[(&str, &str)]) -> LilyFile {
        let mut lily = LilyFile::default();
        lily.variables = variables
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        lily.variables
            .insert("Client Full Name".to_string(), name.to_string());
        lily
    }

    #[test]
    fn merge_takes_only_variables_and_bindings() {
        let mut current = client("CURRENT", &[("Client County", "Denver")]);
        current.questionnaire_id = Some("questionnaire-1".to_string());
        let mut proposal = client("PROPOSED", &[("Client County", "Arapahoe")]);
        proposal
            .variables
            .insert("Client City".to_string(), "Aurora".to_string());
        proposal
            .contact_bindings
            .insert(
                "Personal Representative".to_string(),
                crate::lily_file::ContactBinding {
                    contact_id: None,
                    variable_mappings: Default::default(),
                    contact_ids: None,
                    assignments: None,
                },
            );
        let merged = merge_proposal(&current, &proposal);
        assert_eq!(merged.variables.get("Client Full Name").unwrap(), "PROPOSED");
        assert_eq!(merged.variables.get("Client County").unwrap(), "Arapahoe");
        assert_eq!(merged.variables.get("Client City").unwrap(), "Aurora");
        assert_eq!(merged.questionnaire_id.as_deref(), Some("questionnaire-1"));
        assert!(merged.contact_bindings.contains_key("Personal Representative"));
    }

    #[test]
    fn flags_accept_an_array_or_an_object() {
        let directory = std::env::temp_dir().join(format!(
            "lily-agent-flags-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("flags.json");
        fs::write(
            &path,
            r#"[{"severity":"gap","message":"No guardian assigned"}]"#,
        )
        .unwrap();
        assert_eq!(read_flags(&path).len(), 1);
        fs::write(
            &path,
            r#"{"flags":[{"severity":"info","message":"Checked"}]}"#,
        )
        .unwrap();
        assert_eq!(read_flags(&path).len(), 1);
        fs::remove_dir_all(&directory).ok();
    }

    #[cfg(unix)]
    #[test]
    fn agent_loop_renders_a_proposal_and_approves_it() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use zip::write::{SimpleFileOptions, ZipWriter};

        fn write_minimal_docx(path: &Path, text: &str) {
            let file = fs::File::create(path).expect("docx file");
            let mut writer = ZipWriter::new(file);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            writer
                .start_file("word/document.xml", options)
                .expect("document entry");
            let xml = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body><w:p><w:r><w:t>{}</w:t></w:r></w:p><w:sectPr/></w:body></w:document>",
                text
            );
            writer.write_all(xml.as_bytes()).expect("document xml");
            writer.finish().expect("docx finish");
        }

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!("lily-agent-loop-{}", unique));
        let client_dir = directory.join("client");
        fs::create_dir_all(&client_dir).expect("client directory");
        fs::write(
            client_dir.join("Client.lily"),
            r#"{"lily_type":"client","lily_version":5,"variables":{"Client Full Name":"Jane Doe"},"documents":{"Will.docx":{"template_rel_path":"Will.docx","created_at":"2026-01-01T00:00:00Z","modified_at":"2026-01-01T00:00:00Z"}},"contacts":[],"contact_bindings":{}}"#,
        )
        .expect("client file");
        let skeleton = directory.join("skeleton.docx");
        write_minimal_docx(&skeleton, "Old text");
        let document = client_dir.join("Will.docx");
        fs::copy(&skeleton, &document).expect("document copy");
        let plan_path = directory.join("plan.json");
        fs::write(
            &plan_path,
            r#"{"lily_type":"document-plan","id":"agent-test","version":1,"layout":"skeleton.docx","blocks":[{"id":"block-000","kind":"paragraph","style":"Normal","content":[{"text":"Hello "},{"slot":"Client Full Name"}]}]}"#,
        )
        .expect("plan file");

        let script = directory.join("fake-agent.sh");
        fs::write(
            &script,
            "#!/bin/sh
cat > \"$LILY_AGENT_OUT/proposal.lily\" <<'EOF'
{\"lily_type\":\"client\",\"lily_version\":5,\"variables\":{\"Client Full Name\":\"PROPOSED\"},\"documents\":{},\"contacts\":[],\"contact_bindings\":{}}\nEOF
cat > \"$LILY_AGENT_OUT/flags.json\" <<'EOF'
[{\"severity\":\"info\",\"message\":\"Fake agent ran\"}]\nEOF
echo 'Changed the client name.' > \"$LILY_AGENT_OUT/summary.md\"
",
        )
        .expect("script file");
        let mut permissions = fs::metadata(&script).expect("script metadata").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions).expect("script permissions");

        let report = run_draft_agent(
            document.to_string_lossy().to_string(),
            plan_path.to_string_lossy().to_string(),
            skeleton.to_string_lossy().to_string(),
            Some(script.to_string_lossy().to_string()),
            None,
            Some(directory.to_string_lossy().to_string()),
        )
        .expect("agent loop");
        assert_eq!(report.flags.len(), 1);
        assert!(report.summary.contains("Changed"));
        assert!(Path::new(&report.proposal_path).is_file());
        assert!(Path::new(&report.candidate_path).is_file());
        assert!(report.candidate_html.contains("PROPOSED"));

        approve_agent_candidate(
            document.to_string_lossy().to_string(),
            report.candidate_path.clone(),
            client_dir.to_string_lossy().to_string(),
            report.proposal_path.clone(),
            Some("JANE ATTORNEY".to_string()),
        )
        .expect("approve proposal");
        let lily = lily_file::read_lily_file(&client_dir.to_string_lossy()).expect("client");
        assert_eq!(lily.variables.get("Client Full Name").unwrap(), "PROPOSED");
        let record = lily
            .documents
            .get("Will.docx")
            .and_then(|meta| meta.provenance.as_ref())
            .expect("agent provenance recorded");
        let agent = record.agent.as_ref().expect("agent record");
        assert_eq!(agent.command, script.to_string_lossy());
        assert_eq!(agent.flags.total, 1);
        assert_eq!(record.approval.as_ref().expect("approval").method, "agent-proposal");
        assert_eq!(
            record.approval.as_ref().expect("approval").approved_by.as_deref(),
            Some("JANE ATTORNEY")
        );
        assert!(fs::read_dir(&client_dir)
            .expect("client entries")
            .any(|entry| entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .contains(".bak-")));

        fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn prompt_names_the_three_outputs() {
        let text = prompt(
            Path::new("/client"),
            Path::new("/plan.json"),
            Path::new("/doc.docx"),
            Path::new("/out"),
            Some(Path::new("/lily")),
        );
        assert!(text.contains("proposal.lily"));
        assert!(text.contains("flags.json"));
        assert!(text.contains("summary.md"));
        assert!(text.contains("/lily"));
    }
}
