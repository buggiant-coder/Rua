use rua_skills::{builtin_skill_names, parse_skill_md, BUILTIN_SKILLS};

#[test]
fn all_builtins_parse_with_valid_schema() {
    let names = builtin_skill_names();
    assert_eq!(names.len(), 17, "expected 17 builtin skills");

    for name in &names {
        let dir = BUILTIN_SKILLS
            .get_dir(name)
            .unwrap_or_else(|| panic!("BUILTIN_SKILLS missing dir: {name}"));

        let skill_md_file = dir
            .get_file(format!("{name}/SKILL.md"))
            .unwrap_or_else(|| panic!("{name}/SKILL.md not found in embedded dir"));

        let content = std::str::from_utf8(skill_md_file.contents())
            .unwrap_or_else(|_| panic!("{name}/SKILL.md is not valid UTF-8"));

        let parsed = parse_skill_md(content)
            .unwrap_or_else(|e| panic!("{name}/SKILL.md frontmatter parse failed: {e}"));

        assert!(
            !parsed.description.is_empty(),
            "{name}: description must not be empty"
        );

        assert!(
            !parsed.name.is_empty(),
            "{name}: frontmatter must declare a non-empty 'name' field"
        );
    }
}

#[test]
fn each_builtin_has_non_empty_body() {
    let names = builtin_skill_names();
    for name in &names {
        let dir = BUILTIN_SKILLS.get_dir(name).unwrap();
        let file = dir.get_file(format!("{name}/SKILL.md")).unwrap();
        let content = std::str::from_utf8(file.contents()).unwrap();
        let parsed = parse_skill_md(content).unwrap();
        assert!(
            !parsed.body.trim().is_empty(),
            "{name}: SKILL.md body (markdown after frontmatter) must not be empty"
        );
    }
}

#[test]
fn frontmatter_map_contains_name_and_description_keys() {
    let names = builtin_skill_names();
    for name in &names {
        let dir = BUILTIN_SKILLS.get_dir(name).unwrap();
        let file = dir.get_file(format!("{name}/SKILL.md")).unwrap();
        let content = std::str::from_utf8(file.contents()).unwrap();
        let parsed = parse_skill_md(content).unwrap();

        assert!(
            parsed.frontmatter.contains_key("name"),
            "{name}: frontmatter map must contain 'name' key"
        );
        assert!(
            parsed.frontmatter.contains_key("description"),
            "{name}: frontmatter map must contain 'description' key"
        );
    }
}

#[test]
fn negative_missing_name_field_errors_or_empty() {
    let bad_content = "---\ndescription: no name here\n---\n# body\n";
    let parsed = parse_skill_md(bad_content).expect("parse should succeed (name just empty)");
    assert!(
        parsed.name.is_empty(),
        "missing name field should result in empty name"
    );
}

#[test]
fn negative_invalid_yaml_errors() {
    let bad_yaml = "---\n: :\n  - [unterminated\n---\n# body\n";
    let result = parse_skill_md(bad_yaml);
    assert!(
        result.is_err(),
        "invalid YAML frontmatter must produce an error"
    );
}
