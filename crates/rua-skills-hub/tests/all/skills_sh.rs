use rua_skills_hub::resolve_bundle_source;
use rua_skills_hub::sources::skills_sh::{extract_skills_sh_spec, skills_sh_resolve};
use rua_skills_hub::types::{BundleSource, HubError};

#[test]
fn resolve_skills_sh_url_yields_skills_sh_source() {
    let url = "https://skills.sh/openclaw/skills/foo";
    let source = resolve_bundle_source(url).expect("resolve");
    assert_eq!(source, BundleSource::SkillsSh(url.into()));
}

#[test]
fn extract_spec_picks_owner_repo_skill() {
    let (owner, repo, skill) =
        extract_skills_sh_spec("https://skills.sh/owner/repo/cool-skill").expect("ok");
    assert_eq!(owner, "owner");
    assert_eq!(repo, "repo");
    assert_eq!(skill, "cool-skill");

    let (_owner, _repo, _skill) =
        extract_skills_sh_spec("https://www.skills.sh/o/r/s").expect("ok");
}

#[test]
fn extract_spec_rejects_short_paths() {
    assert!(extract_skills_sh_spec("https://skills.sh/owner/repo").is_none());
    assert!(extract_skills_sh_spec("https://skills.sh/owner").is_none());
    assert!(extract_skills_sh_spec("https://skills.sh/").is_none());

    assert!(extract_skills_sh_spec("https://example.com/o/r/s").is_none());

    assert!(extract_skills_sh_spec("https://skills.sh/o//s").is_none());
}

#[test]
fn resolve_dispatches_to_github_spec() {
    let (spec, skill) =
        skills_sh_resolve("https://skills.sh/openclaw/skills-repo/himalaya").expect("resolve");
    assert_eq!(spec.owner, "openclaw");
    assert_eq!(spec.repo, "skills-repo");

    assert_eq!(spec.path, "himalaya");
    assert_eq!(spec.branch, "");
    assert_eq!(skill, "himalaya");
}

#[test]
fn resolve_rejects_invalid_url() {
    let err = skills_sh_resolve("https://skills.sh/owner/repo").unwrap_err();
    matches!(err, HubError::InvalidBundle(_))
        .then_some(())
        .expect("must be InvalidBundle");
}
