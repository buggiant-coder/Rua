use std::path::{Path, PathBuf};

use rua_core::{Result, RuaError};

use super::{SeedCoordinator, SeedProducer, SeedRunReport};

const SKILL_POOL_PRODUCER: &str = "builtin_skill_pool";

pub fn ensure_builtin_skill_pool_seeded(data_dir: &Path) -> SeedRunReport {
    let mut names = rua_skills::builtin_skill_names();
    names.sort();
    let mut coordinator = SeedCoordinator::new(data_dir);
    for name in names {
        coordinator = coordinator.with_producer(Box::new(BuiltinSkillSeedProducer { name }));
    }
    coordinator.run()
}

struct BuiltinSkillSeedProducer {
    name: String,
}

impl BuiltinSkillSeedProducer {
    fn pool_dir(data_dir: &Path) -> PathBuf {
        data_dir.join("skill_pool")
    }

    fn skill_dir(&self, data_dir: &Path) -> PathBuf {
        Self::pool_dir(data_dir).join(&self.name)
    }

    fn skill_dir_rel_path(&self) -> String {
        format!("skill_pool/{}", self.name)
    }
}

impl SeedProducer for BuiltinSkillSeedProducer {
    fn entry_id(&self) -> String {
        format!("skill_pool:{}", self.name)
    }

    fn producer_name(&self) -> &str {
        SKILL_POOL_PRODUCER
    }

    fn target_paths(&self) -> Vec<String> {
        vec![self.skill_dir_rel_path()]
    }

    fn materialize(&self, data_dir: &Path) -> Result<Vec<String>> {
        let pool_dir = Self::pool_dir(data_dir);
        let selected = vec![self.name.clone()];
        rua_skills::import_builtin_skills_in(&pool_dir, Some(&selected), false).map_err(|e| {
            RuaError::Config {
                message: format!("Failed to import builtin skill {}: {e}", self.name),
            }
        })?;

        collect_relative_files(data_dir, &self.skill_dir(data_dir))
    }
}

fn collect_relative_files(data_dir: &Path, root: &Path) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    collect_files(root, &mut paths).map_err(|e| RuaError::Io {
        message: format!(
            "Failed to collect seeded skill files {}: {e}",
            root.display()
        ),
    })?;
    paths.sort();
    let mut rels = Vec::with_capacity(paths.len());
    for path in paths {
        let rel = path
            .strip_prefix(data_dir)
            .map_err(|e| RuaError::Validation {
                message: format!("Failed to relativize seeded skill path: {e}"),
            })?;
        rels.push(rel.to_string_lossy().replace('\\', "/"));
    }
    Ok(rels)
}

fn collect_files(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    if !root.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}
