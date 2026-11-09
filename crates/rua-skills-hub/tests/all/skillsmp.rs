use rua_skills_hub::resolve_bundle_source;
use rua_skills_hub::sources::skillsmp::{
    extract_skillsmp_slug, extract_skillsmp_spec_conservative,
};
use rua_skills_hub::types::BundleSource;

#[test]
fn resolve_skillsmp_url_yields_skillsmp_source() {
    let url = "https://skillsmp.com/skills/openclaw-foo-bar";
    let source = resolve_bundle_source(url).expect("resolve");
    assert_eq!(source, BundleSource::SkillsMp(url.into()));
}

#[test]
fn extract_slug_from_skills_path_form() {
    assert_eq!(
        extract_skillsmp_slug("https://skillsmp.com/skills/openclaw-foo-bar"),
        Some("openclaw-foo-bar".to_string()),
    );
    assert_eq!(
        extract_skillsmp_slug("https://www.skillsmp.com/skills/abc-def"),
        Some("abc-def".to_string()),
    );

    assert_eq!(extract_skillsmp_slug("https://example.com/skills/x"), None);

    assert_eq!(extract_skillsmp_slug("https://skillsmp.com/skills/"), None);
}

#[test]
fn conservative_spec_strips_skill_md_and_splits_at_first_token() {
    let url = "https://skillsmp.com/skills/openclaw-openclaw-skills-himalaya-skill-md";
    let (owner, repo, skill_hint) =
        extract_skillsmp_spec_conservative(url).expect("conservative spec");
    assert_eq!(owner, "openclaw");

    assert_eq!(repo, "openclaw");
    assert_eq!(skill_hint, "skills-himalaya");
}

#[test]
fn spec_rejects_too_few_tokens() {
    assert!(extract_skillsmp_spec_conservative("https://skillsmp.com/skills/just-two").is_none());
    assert!(extract_skillsmp_spec_conservative("https://skillsmp.com/skills/single").is_none());
}

#[test]
fn spec_rejects_non_skillsmp_host() {
    assert!(
        extract_skillsmp_spec_conservative("https://example.com/skills/openclaw-repo-skill")
            .is_none()
    );
}
