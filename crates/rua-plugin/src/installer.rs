use std::path::{Path, PathBuf};

use rua_core::runtime_authority::{
    ProcessShape, RuntimeAuthority, RuntimeLaunchPlan, RuntimeLaunchRequest, ToolchainIntent,
};
use rua_core::{Result, RuaError};

use crate::{InstallRequest, PluginManager, PluginRecord};

#[derive(Debug, Clone)]
pub struct PipInstallResult {
    pub plugin_dir: PathBuf,
    pub manifest_id: String,
    pub manifest_name: String,
    pub manifest_version: String,
}

pub struct PluginInstaller {
    plugins_base_dir: PathBuf,
    runtime_cache_dir: PathBuf,
}

impl PluginInstaller {
    pub fn new(plugins_base_dir: PathBuf) -> Self {
        Self {
            runtime_cache_dir: std::env::temp_dir().join("rua-runtime-cache"),
            plugins_base_dir,
        }
    }

    pub fn with_runtime_cache_dir(mut self, runtime_cache_dir: PathBuf) -> Self {
        self.runtime_cache_dir = runtime_cache_dir;
        self
    }

    pub async fn install(
        &self,
        plugin_id: &str,
        package_spec: &str,
        manager: &PluginManager,
    ) -> Result<PluginRecord> {
        let target_dir = self.plugins_base_dir.join(plugin_id);
        std::fs::create_dir_all(&target_dir).map_err(|e| RuaError::Io {
            message: format!(
                "failed to create plugin directory {}: {}",
                target_dir.display(),
                e
            ),
        })?;

        self.pip_install(package_spec, &target_dir).await?;

        let manifest = self.parse_manifest(&target_dir)?;

        let record = manager
            .install(InstallRequest {
                plugin_id: plugin_id.to_string(),
                version: Some(manifest.version.clone()),
                source_url: None,
                extra: {
                    let mut extra = serde_json::Map::new();
                    extra.insert(
                        "source_path".into(),
                        serde_json::Value::String(target_dir.display().to_string()),
                    );
                    extra.insert(
                        "package_spec".into(),
                        serde_json::Value::String(package_spec.to_string()),
                    );
                    extra.insert(
                        "entry_point".into(),
                        serde_json::Value::String(manifest.entry_point.clone()),
                    );
                    if !manifest.name.is_empty() {
                        extra.insert(
                            "author".into(),
                            serde_json::Value::String(manifest.author.clone()),
                        );
                    }
                    extra
                },
            })
            .await?;

        Ok(record)
    }

    async fn pip_install(&self, package_spec: &str, target_dir: &Path) -> Result<()> {
        let plan =
            plugin_pip_install_launch_plan(package_spec, target_dir, &self.runtime_cache_dir);
        let process = plan.into_process()?;
        let output = tokio::process::Command::new(process.command)
            .args(process.args)
            .envs(process.env)
            .current_dir(process.cwd.unwrap_or_else(|| target_dir.to_path_buf()))
            .output()
            .await
            .map_err(|e| RuaError::Runtime {
                message: format!("failed to run plugin pip install: {}", e),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuaError::Runtime {
                message: format!(
                    "pip install failed (exit {}): {}",
                    output.status.code().unwrap_or(-1),
                    stderr.trim()
                ),
            });
        }

        Ok(())
    }

    fn parse_manifest(&self, plugin_dir: &Path) -> Result<PluginManifestInfo> {
        let manifest_path = plugin_dir.join("plugin.json");
        if !manifest_path.exists() {
            return Err(RuaError::NotFound {
                resource: "manifest".into(),
                id: manifest_path.display().to_string(),
            });
        }

        let data = std::fs::read_to_string(&manifest_path).map_err(|e| RuaError::Io {
            message: format!("failed to read manifest: {}", e),
        })?;

        let manifest: PluginManifestInfo =
            serde_json::from_str(&data).map_err(|e| RuaError::Runtime {
                message: format!("failed to parse manifest: {}", e),
            })?;

        Ok(manifest)
    }
}

#[doc(hidden)]
pub fn plugin_pip_install_launch_plan(
    package_spec: &str,
    target_dir: &Path,
    runtime_cache_dir: &Path,
) -> RuntimeLaunchPlan {
    let authority = RuntimeAuthority::from_current_process(None, target_dir, runtime_cache_dir);
    plugin_pip_install_launch_plan_with_authority(package_spec, target_dir, &authority)
}

#[doc(hidden)]
pub fn plugin_pip_install_launch_plan_with_authority(
    package_spec: &str,
    target_dir: &Path,
    authority: &RuntimeAuthority,
) -> RuntimeLaunchPlan {
    authority.plan(
        ProcessShape::Install,
        ToolchainIntent::Python,
        RuntimeLaunchRequest {
            args: vec![
                "-m".to_string(),
                "pip".to_string(),
                "install".to_string(),
                "--target".to_string(),
                target_dir.to_string_lossy().into_owned(),
                package_spec.to_string(),
            ],
            cwd: Some(target_dir.to_path_buf()),
            ..Default::default()
        },
    )
}

#[derive(Debug, Clone, serde::Deserialize)]
struct PluginManifestInfo {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_entry_point")]
    pub entry_point: String,
}

fn default_entry_point() -> String {
    "plugin.wasm".into()
}
