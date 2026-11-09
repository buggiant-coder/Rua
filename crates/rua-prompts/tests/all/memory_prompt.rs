#[test]
fn structured_memory_is_an_independent_builtin_prompt_file() {
    let prompt = rua_prompts::builtin::BUILTIN_PROMPTS
        .iter()
        .find(|prompt| prompt.path == "40_STRUCTURED_MEMORY.md")
        .expect("Structured Memory prompt");
    assert!(
        prompt.content.contains("memory_search")
            && prompt.content.contains("memory_get")
            && prompt.content.contains("memory_write")
            && prompt.content.contains("memory_update")
            && prompt.content.contains("memory_delete")
            && prompt
                .content
                .contains("Never create, read, edit, grep, repair, or delete Memory"),
        "Structured Memory prompt must teach the complete tool protocol"
    );
    assert!(
        rua_prompts::builtin::BUILTIN_PROMPTS
            .iter()
            .all(|prompt| prompt.path != "AGENT_MEMORY.md"),
        "legacy AGENT_MEMORY.md must stay absent"
    );
}

#[test]
fn markdown_plus_is_an_independent_builtin_prompt_file() {
    let prompt = rua_prompts::builtin::BUILTIN_PROMPTS
        .iter()
        .find(|prompt| prompt.path == "60_MARKDOWN_PLUS.md")
        .expect("Markdown Plus prompt");
    assert!(prompt.content.contains("fenced Mermaid"));
    assert!(prompt.content.contains("@figure"));
}

#[test]
fn builtin_prompt_prose_is_not_hard_wrapped() {
    for prompt in rua_prompts::builtin::BUILTIN_PROMPTS {
        assert!(
            prompt.content.starts_with("# "),
            "{} must own its top-level heading",
            prompt.path
        );
        let mut in_fence = false;
        let mut previous_was_prose = false;
        for (index, line) in prompt.content.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
                previous_was_prose = false;
                continue;
            }
            if in_fence {
                continue;
            }
            assert!(
                !line.starts_with(' '),
                "{}:{} contains an indented prose continuation",
                prompt.path,
                index + 1
            );
            let is_structure = trimmed.is_empty()
                || trimmed.starts_with('#')
                || trimmed.starts_with("- ")
                || trimmed.starts_with("> ")
                || trimmed.starts_with('@')
                || trimmed == "$$"
                || trimmed
                    .split_once(". ")
                    .is_some_and(|(prefix, _)| prefix.chars().all(|ch| ch.is_ascii_digit()));
            let is_prose = !is_structure;
            assert!(
                !(previous_was_prose && is_prose),
                "{}:{} contains a hard-wrapped prose paragraph",
                prompt.path,
                index + 1
            );
            previous_was_prose = is_prose;
        }
    }
}

#[test]
fn other_builtin_prompts_do_not_teach_removed_memory_markdown_storage() {
    for prompt in rua_prompts::builtin::BUILTIN_PROMPTS.iter() {
        for forbidden in [
            "memory/YYYY-MM-DD.md",
            "memory/*.md",
            "更新 `MEMORY.md`",
            "写入 MEMORY.md",
            "读 MEMORY.md",
            "读取 `MEMORY.md`",
            "MEMORY.md 与 `memory/`",
            "本工作区的MEMORY.md",
            "关键文件：** `config.json`、`AGENTS.md`、`MEMORY.md`",
            "AGENTS.md、MEMORY.md",
        ] {
            assert!(
                !prompt.content.contains(forbidden),
                "{} must not teach removed Memory Markdown storage via `{}`",
                prompt.path,
                forbidden
            );
        }
    }
}
