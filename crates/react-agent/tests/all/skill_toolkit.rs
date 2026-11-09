use std::path::PathBuf;

use agent_types::ToolCall;
use rua_core::tool::ToolContext;

use react_agent::skill_tool::{build_skills_prompt_block, register_skill_pseudo_tool, SkillInfo};
use react_agent::toolkit::AgentToolkit;
use react_agent::SkillRuntimeSource;

fn dummy_ctx() -> ToolContext {
    ToolContext {
        agent_id: "test-agent".to_string(),
        session_id: "test-session".to_string(),
        working_dir: std::env::temp_dir(),
        metadata: Default::default(),
        cancel: None,
        task_runtime: None,

        ..Default::default()
    }
}

fn make_skill(name: &str, description: &str, body: &str) -> SkillInfo {
    SkillInfo {
        name: name.to_string(),
        description: description.to_string(),
        body: body.to_string(),
        source: SkillRuntimeSource::Agent,
        base_dir: PathBuf::from("/nonexistent-skill-test"),
    }
}

#[test]
fn u1_registers_single_skill_tool_with_one_skill() {
    let mut toolkit = AgentToolkit::new();
    register_skill_pseudo_tool(
        &mut toolkit,
        vec![make_skill("pdf", "Read PDF files", "body-pdf")],
    )
    .unwrap();

    let names = toolkit.tool_names();
    assert_eq!(
        names,
        vec!["skill"],
        "toolkit must have exactly [\"skill\"]"
    );

    let def = toolkit.get_definition("skill").unwrap();
    assert_eq!(def.name, "skill");

    let schema = &def.parameters;
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["name"]["type"], "string");
    assert_eq!(schema["required"], serde_json::json!(["name"]));
    assert_eq!(schema["additionalProperties"], false);

    assert!(
        def.description.contains("pdf"),
        "description must list skill name"
    );
    assert!(
        def.description.contains("Read PDF files"),
        "description must list skill description"
    );
}

#[test]
fn u2_registers_single_skill_tool_with_three_skills() {
    let mut toolkit = AgentToolkit::new();
    register_skill_pseudo_tool(
        &mut toolkit,
        vec![
            make_skill("pdf", "Read PDF files", "body-pdf"),
            make_skill("docx", "Work with Word documents", "body-docx"),
            make_skill("xlsx", "Read Excel spreadsheets", "body-xlsx"),
        ],
    )
    .unwrap();

    let mut names = toolkit.tool_names();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["skill"],
        "must be exactly one tool named 'skill'"
    );

    let def = toolkit.get_definition("skill").unwrap();
    let desc = &def.description;
    let pos_pdf = desc.find("pdf").expect("pdf must appear");
    let pos_docx = desc.find("docx").expect("docx must appear");
    let pos_xlsx = desc.find("xlsx").expect("xlsx must appear");
    assert!(
        pos_pdf < pos_docx,
        "pdf must come before docx (input order)"
    );
    assert!(
        pos_docx < pos_xlsx,
        "docx must come before xlsx (input order)"
    );
}

#[test]
fn u3_does_not_register_tool_when_skills_empty() {
    let mut toolkit = AgentToolkit::new();
    let result = register_skill_pseudo_tool(&mut toolkit, vec![]);
    assert!(result.is_ok());
    assert!(
        !toolkit.has_tool("skill"),
        "empty skills must not register a tool"
    );
    assert!(toolkit.is_empty());
}

#[test]
fn u4_prompt_block_returns_explicit_no_skills_signal_when_empty() {
    let block = build_skills_prompt_block(&[]).expect("empty skills still produce guard block");
    assert!(block.contains("<available_skills>"));
    assert!(block.contains("No skills are installed or enabled"));
    assert!(block.contains("</available_skills>"));
}

#[test]
fn u5_prompt_block_preserves_source_and_path_identity() {
    let skills = vec![
        make_skill("pdf", "Read, extract text and tables from PDF files", "b"),
        make_skill("docx", "Work with Microsoft Word documents", "b"),
        make_skill("xlsx", "Read and write Excel spreadsheets", "b"),
    ];
    let block = build_skills_prompt_block(&skills).unwrap();
    let expected = "\
<available_skills>
Skills provide specialized instructions. Load via skill(name=\"...\") when relevant.

- `pdf`: Read, extract text and tables from PDF files
- `docx`: Work with Microsoft Word documents
- `xlsx`: Read and write Excel spreadsheets
</available_skills>";
    let expected = expected
        .replace(
            "- `pdf`:",
            "- `pdf` (source `agent`, path `/nonexistent-skill-test`):",
        )
        .replace(
            "- `docx`:",
            "- `docx` (source `agent`, path `/nonexistent-skill-test`):",
        )
        .replace(
            "- `xlsx`:",
            "- `xlsx` (source `agent`, path `/nonexistent-skill-test`):",
        );
    assert_eq!(block, expected);
}

#[tokio::test]
async fn u6_invoking_skill_with_known_name_returns_body() {
    let mut toolkit = AgentToolkit::new();
    register_skill_pseudo_tool(
        &mut toolkit,
        vec![
            make_skill("skill-a", "Skill A", "body of skill-a"),
            make_skill("skill-b", "Skill B", "body of skill-b"),
        ],
    )
    .unwrap();

    let executor = toolkit.get_executor("skill").unwrap();
    let call = ToolCall::new("call-u6", "skill", serde_json::json!({"name": "skill-a"}));
    let ctx = dummy_ctx();
    let result = executor.execute(&call, &ctx).await.unwrap();

    assert!(!result.is_error, "known skill must not set is_error");
    let content = result.text_content();
    assert!(
        content.contains("body of skill-a"),
        "must contain body: {content}"
    );
    assert!(
        content.contains("Base directory:"),
        "must contain base dir: {content}"
    );
}

#[tokio::test]
async fn u7_invoking_skill_with_unknown_name_returns_is_error_with_available_list() {
    let mut toolkit = AgentToolkit::new();
    register_skill_pseudo_tool(
        &mut toolkit,
        vec![
            make_skill("skill-a", "Skill A", "body-a"),
            make_skill("skill-b", "Skill B", "body-b"),
        ],
    )
    .unwrap();

    let executor = toolkit.get_executor("skill").unwrap();
    let call = ToolCall::new("call-u7", "skill", serde_json::json!({"name": "nope"}));
    let ctx = dummy_ctx();
    let result = executor.execute(&call, &ctx).await.unwrap();

    assert!(result.is_error, "unknown skill must set is_error");
    let body = result.text_content();
    assert!(body.contains("Unknown skill 'nope'"), "body: {body}");
    assert!(body.contains("skill-a"), "body must list available: {body}");
    assert!(body.contains("skill-b"), "body must list available: {body}");
}

#[tokio::test]
async fn u8_invoking_skill_with_missing_name_arg_returns_is_error() {
    let mut toolkit = AgentToolkit::new();
    register_skill_pseudo_tool(
        &mut toolkit,
        vec![
            make_skill("skill-a", "Skill A", "body-a"),
            make_skill("skill-b", "Skill B", "body-b"),
        ],
    )
    .unwrap();

    let executor = toolkit.get_executor("skill").unwrap();
    let call = ToolCall::new("call-u8", "skill", serde_json::json!({}));
    let ctx = dummy_ctx();
    let result = executor.execute(&call, &ctx).await.unwrap();

    assert!(result.is_error, "missing name arg must set is_error");
    let body = result.text_content();
    assert!(body.contains("Unknown skill '<missing>'"), "body: {body}");
    assert!(
        body.contains("skill-a") && body.contains("skill-b"),
        "body: {body}"
    );
}

#[tokio::test]
async fn u9_invoking_skill_with_non_string_name_returns_is_error() {
    let mut toolkit = AgentToolkit::new();
    register_skill_pseudo_tool(
        &mut toolkit,
        vec![make_skill("skill-a", "Skill A", "body-a")],
    )
    .unwrap();

    let executor = toolkit.get_executor("skill").unwrap();
    let call = ToolCall::new("call-u9", "skill", serde_json::json!({"name": 42}));
    let ctx = dummy_ctx();
    let result = executor.execute(&call, &ctx).await.unwrap();

    assert!(result.is_error, "non-string name must set is_error");
    let body = result.text_content();
    assert!(
        body.to_lowercase().contains("invalid") || body.to_lowercase().contains("expected"),
        "error must mention arg validation; got: {body}"
    );
}

#[tokio::test]
async fn u10_duplicate_skill_names_require_path_qualified_resolution() {
    let mut toolkit = AgentToolkit::new();
    let mut first = make_skill("dup-skill", "First", "first-body");
    first.base_dir = PathBuf::from("/skills/first");
    let mut second = make_skill("dup-skill", "Second", "second-body");
    second.base_dir = PathBuf::from("/skills/second");
    register_skill_pseudo_tool(&mut toolkit, vec![first, second]).unwrap();

    let executor = toolkit.get_executor("skill").unwrap();
    let ambiguous = ToolCall::new(
        "call-u10",
        "skill",
        serde_json::json!({"name": "dup-skill"}),
    );
    let ctx = dummy_ctx();
    let result = executor.execute(&ambiguous, &ctx).await.unwrap();

    assert!(result.is_error);
    let content = result.text_content();
    assert!(content.contains("Ambiguous skill 'dup-skill'"));
    assert!(content.contains("path:/skills/first"));
    assert!(content.contains("path:/skills/second"));

    let qualified = ToolCall::new(
        "call-u10-qualified",
        "skill",
        serde_json::json!({"name": "path:/skills/second"}),
    );
    let result = executor.execute(&qualified, &ctx).await.unwrap();
    assert!(!result.is_error);
    let content = result.text_content();
    assert!(
        content.contains("second-body"),
        "qualified entry must load: {content}"
    );
}

#[tokio::test]
async fn u13_agent_and_project_same_name_resolve_by_source() {
    let mut agent = make_skill("review", "Agent review", "agent-body");
    agent.base_dir = PathBuf::from("/agent/skills/review");
    let mut project = make_skill("review", "Project review", "project-body");
    project.source = SkillRuntimeSource::Project;
    project.base_dir = PathBuf::from("/repo/.agents/skills/review");
    let mut toolkit = AgentToolkit::new();
    register_skill_pseudo_tool(&mut toolkit, vec![agent, project]).unwrap();
    let executor = toolkit.get_executor("skill").unwrap();
    let ctx = dummy_ctx();

    let ambiguous = executor
        .execute(
            &ToolCall::new(
                "call-u13-ambiguous",
                "skill",
                serde_json::json!({"name": "review"}),
            ),
            &ctx,
        )
        .await
        .unwrap();
    assert!(ambiguous.is_error);

    let agent_result = executor
        .execute(
            &ToolCall::new(
                "call-u13-agent",
                "skill",
                serde_json::json!({"name": "agent:review"}),
            ),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!agent_result.is_error);
    assert!(agent_result.text_content().contains("agent-body"));
    assert!(agent_result.text_content().contains("Source: agent"));

    let project_result = executor
        .execute(
            &ToolCall::new(
                "call-u13-project",
                "skill",
                serde_json::json!({"name": "project:review"}),
            ),
            &ctx,
        )
        .await
        .unwrap();
    assert!(!project_result.is_error);
    assert!(project_result.text_content().contains("project-body"));
    assert!(project_result.text_content().contains("Source: project"));
    assert!(project_result.metadata.is_empty());
}

#[tokio::test]
async fn u11_skill_result_includes_base_dir_and_file_list() {
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().to_path_buf();

    std::fs::write(skill_dir.join("SKILL.md"), "# My Skill").unwrap();

    std::fs::write(skill_dir.join("CONTEXT-FORMAT.md"), "context format").unwrap();
    std::fs::write(skill_dir.join("template.txt"), "template content").unwrap();

    std::fs::create_dir(skill_dir.join("references")).unwrap();
    std::fs::write(
        skill_dir.join("references").join("guide.md"),
        "guide content",
    )
    .unwrap();

    let skill = SkillInfo {
        name: "test-skill".to_string(),
        description: "A test skill".to_string(),
        body: "skill body content".to_string(),
        source: SkillRuntimeSource::Agent,
        base_dir: skill_dir.clone(),
    };

    let mut toolkit = AgentToolkit::new();
    register_skill_pseudo_tool(&mut toolkit, vec![skill]).unwrap();

    let executor = toolkit.get_executor("skill").unwrap();
    let call = ToolCall::new(
        "call-u11",
        "skill",
        serde_json::json!({"name": "test-skill"}),
    );
    let ctx = dummy_ctx();
    let result = executor.execute(&call, &ctx).await.unwrap();

    assert!(!result.is_error);
    let content = result.text_content();

    assert!(
        content.contains("skill body content"),
        "must contain body: {content}"
    );

    let dir_str = skill_dir.display().to_string();
    assert!(
        content.contains(&format!("Base directory: {}", dir_str)),
        "must contain base dir: {content}"
    );
    assert!(
        !content.contains("file://"),
        "must use bare path, not file:// URI: {content}"
    );

    assert!(
        content.contains("<skill_files>"),
        "must have skill_files block: {content}"
    );
    assert!(
        content.contains("CONTEXT-FORMAT.md"),
        "must list CONTEXT-FORMAT.md: {content}"
    );
    assert!(
        content.contains("template.txt"),
        "must list template.txt: {content}"
    );
    assert!(
        content.contains("guide.md"),
        "must list nested guide.md: {content}"
    );

    let skill_files_section = content.split("<skill_files>").nth(1).unwrap_or("");
    assert!(
        !skill_files_section.contains("SKILL.md"),
        "SKILL.md must be excluded from file list: {content}"
    );
}

#[tokio::test]
async fn u12_skill_result_no_files_block_when_dir_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let skill_dir = tmp.path().to_path_buf();

    std::fs::write(skill_dir.join("SKILL.md"), "# Empty skill").unwrap();

    let skill = SkillInfo {
        name: "empty-skill".to_string(),
        description: "An empty skill".to_string(),
        body: "empty body".to_string(),
        source: SkillRuntimeSource::Agent,
        base_dir: skill_dir.clone(),
    };

    let mut toolkit = AgentToolkit::new();
    register_skill_pseudo_tool(&mut toolkit, vec![skill]).unwrap();

    let executor = toolkit.get_executor("skill").unwrap();
    let call = ToolCall::new(
        "call-u12",
        "skill",
        serde_json::json!({"name": "empty-skill"}),
    );
    let ctx = dummy_ctx();
    let result = executor.execute(&call, &ctx).await.unwrap();

    assert!(!result.is_error);
    let content = result.text_content();

    assert!(
        content.contains("empty body"),
        "must contain body: {content}"
    );
    assert!(
        content.contains("Base directory:"),
        "must contain base dir even with no files: {content}"
    );
    assert!(
        !content.contains("<skill_files>"),
        "must not have skill_files block when no resource files: {content}"
    );
}
