use std::sync::Arc;

use async_trait::async_trait;
use clap::Args;
use rua_acp::server::{
    AcpApprovalBackend, AcpApprovalOption, AcpApprovalOptionKind, AcpApprovalRequest, RuaAcpServer,
};
use rua_config::dirs::Paths;
use rua_contract::agent::AgentRuntime;
use rua_contract::config::AppConfig;
use rua_security::{ApprovalAuthority, ApprovalDecision, ApprovalRequest, ApprovalReuseScope};
use rua_server::ServerProfile;

use crate::acp_remote::{probe_runtime, HttpAgentRuntime, HttpApprovalBackend};
use crate::http_client::{resolve_base_url, ApiClient};

#[derive(Debug, Args)]
pub struct AcpArgs {
    #[arg(long)]
    agent: Option<String>,

    #[arg(long, requires = "model_id")]
    provider_id: Option<String>,

    #[arg(long, requires = "provider_id")]
    model_id: Option<String>,
}

pub fn run(args: AcpArgs, paths: &Paths, base_url: Option<&str>) -> anyhow::Result<()> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_acp(args, paths, base_url))
}

struct SecurityApprovalBackend {
    authority: Arc<ApprovalAuthority>,
}

impl SecurityApprovalBackend {
    fn new(authority: Arc<ApprovalAuthority>) -> Self {
        Self { authority }
    }

    async fn pending(&self, agent_id: &str, request_id: &str) -> Option<ApprovalRequest> {
        self.authority
            .list_pending_for_agent(agent_id)
            .await
            .into_iter()
            .find(|request| request.id == request_id)
    }
}

#[async_trait]
impl AcpApprovalBackend for SecurityApprovalBackend {
    async fn pending_request(
        &self,
        agent_id: &str,
        session_id: &str,
        request_id: &str,
    ) -> Result<Option<AcpApprovalRequest>, String> {
        let Some(request) = self.pending(agent_id, request_id).await else {
            return Ok(None);
        };
        if !request.owner.is_runtime_session(agent_id, session_id) {
            return Err("approval request belongs to another ACP session".to_string());
        }

        let mut options = vec![AcpApprovalOption {
            id: "run_once".to_string(),
            label: "Allow once".to_string(),
            kind: AcpApprovalOptionKind::AllowOnce,
        }];
        for (index, option) in request.reuse_options.iter().enumerate() {
            let prefix = match option.scope {
                ApprovalReuseScope::Chat => continue,
                ApprovalReuseScope::RuntimeSession => "allow_runtime_session",
                ApprovalReuseScope::Agent => "allow_agent",
            };
            options.push(AcpApprovalOption {
                id: format!("{prefix}:{index}"),
                label: option.label.clone(),
                kind: AcpApprovalOptionKind::AllowAlways,
            });
        }
        options.push(AcpApprovalOption {
            id: "reject".to_string(),
            label: "Reject".to_string(),
            kind: AcpApprovalOptionKind::RejectOnce,
        });

        Ok(Some(AcpApprovalRequest {
            request_id: request.id,
            tool_call_id: request.tool_call_id,
            title: format!("{} requires approval", request.tool_name),
            raw_input: request.tool_call.args,
            options,
        }))
    }

    async fn resolve(
        &self,
        agent_id: &str,
        request_id: &str,
        option_id: Option<&str>,
    ) -> Result<(), String> {
        let request = self
            .pending(agent_id, request_id)
            .await
            .ok_or_else(|| "approval request is no longer pending".to_string())?;
        let decided_by = "acp-client".to_string();
        let decision = match option_id {
            Some("run_once") => ApprovalDecision::RunOnce { decided_by },
            Some(option_id) if option_id.starts_with("allow_runtime_session:") => {
                let index = option_index(option_id, "allow_runtime_session:")?;
                let option = request
                    .reuse_options
                    .get(index)
                    .filter(|option| option.scope == ApprovalReuseScope::RuntimeSession)
                    .ok_or_else(|| "invalid runtime session approval option".to_string())?;
                ApprovalDecision::AllowRuntimeSession {
                    decided_by,
                    matcher: option.matcher.clone(),
                }
            }
            Some(option_id) if option_id.starts_with("allow_agent:") => {
                let index = option_index(option_id, "allow_agent:")?;
                let option = request
                    .reuse_options
                    .get(index)
                    .filter(|option| option.scope == ApprovalReuseScope::Agent)
                    .ok_or_else(|| "invalid agent approval option".to_string())?;
                ApprovalDecision::AllowAgent {
                    decided_by,
                    matcher: option.matcher.clone(),
                }
            }
            _ => ApprovalDecision::Reject { decided_by },
        };
        self.authority
            .decide_for_agent(agent_id, request_id, decision)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }

    async fn cancel_session(&self, agent_id: &str, session_id: &str) -> Result<(), String> {
        self.authority
            .clear_by_runtime_session(agent_id, session_id)
            .await;
        Ok(())
    }
}

fn option_index(option_id: &str, prefix: &str) -> Result<usize, String> {
    option_id
        .strip_prefix(prefix)
        .ok_or_else(|| "invalid ACP approval option".to_string())?
        .parse()
        .map_err(|_| "invalid ACP approval option index".to_string())
}

async fn serve_acp(
    runtime: Arc<dyn AgentRuntime>,
    approval_backend: Arc<dyn AcpApprovalBackend>,
    agent_id: &str,
    selected_model: Option<&(String, String)>,
    source: &str,
) -> anyhow::Result<()> {
    let mut server = RuaAcpServer::new_with_agent_id(runtime, agent_id);
    if let Some((provider_id, model_id)) = selected_model {
        server = server.with_model(provider_id, model_id);
    }
    let server = Arc::new(server.with_approval_backend(approval_backend));

    if let Some((provider_id, model_id)) = selected_model {
        eprintln!(
            "[acp] starting ACP stdio server via {source} for agent '{agent_id}' with model \
             '{provider_id}/{model_id}'"
        );
    } else {
        eprintln!("[acp] starting ACP stdio server via {source} for agent '{agent_id}'");
    }

    rua_acp::server::v1::serve_stdio(server)
        .await
        .map_err(|e| anyhow::anyhow!("ACP server error: {e}"))
}

async fn run_embedded_acp(
    paths: &Paths,
    agent_id: &str,
    selected_model: Option<&(String, String)>,
) -> anyhow::Result<()> {
    let data_dir = paths.rua_root.clone();
    let secret_dir = paths.secret_dir.clone();
    let config = AppConfig::default();

    let state = tokio::task::spawn_blocking(move || {
        rua_server::state::AppState::load(config, data_dir, secret_dir, ServerProfile::Desktop)
    })
    .await
    .map_err(|e| anyhow::anyhow!("AppState construction panicked: {e}"))??;
    let ws_runtime = state.registry.get_or_create(agent_id).await.map_err(|e| {
        anyhow::anyhow!("failed to get workspace runtime for agent '{agent_id}': {e}")
    })?;
    let react_runtime = ws_runtime
        .react_runtime()
        .await
        .ok_or_else(|| anyhow::anyhow!("react runtime not available for agent '{agent_id}'"))?;
    let approval_backend: Arc<dyn AcpApprovalBackend> = Arc::new(SecurityApprovalBackend::new(
        Arc::clone(state.security.approval_authority()),
    ));
    serve_acp(
        react_runtime,
        approval_backend,
        agent_id,
        selected_model,
        "embedded runtime",
    )
    .await
}

async fn run_acp(
    args: AcpArgs,
    paths: &Paths,
    base_url_override: Option<&str>,
) -> anyhow::Result<()> {
    let selected_model = match (args.provider_id, args.model_id) {
        (Some(provider_id), Some(model_id)) => Some((provider_id, model_id)),
        (None, None) => None,
        _ => anyhow::bail!("--provider-id and --model-id must be specified together"),
    };
    let active_agent_id = rua_server::state::AppState::resolve_active_agent_id(&paths.rua_root);
    let agent_id = args.agent.unwrap_or(active_agent_id);
    let base_url = resolve_base_url(base_url_override);
    let client = Arc::new(ApiClient::new(&base_url, &agent_id));

    if probe_runtime(&client, &agent_id).await? {
        let runtime: Arc<dyn AgentRuntime> =
            Arc::new(HttpAgentRuntime::new(Arc::clone(&client), &agent_id));
        let approval_backend: Arc<dyn AcpApprovalBackend> =
            Arc::new(HttpApprovalBackend::new(client));
        return serve_acp(
            runtime,
            approval_backend,
            &agent_id,
            selected_model.as_ref(),
            &format!("Rua server {base_url}"),
        )
        .await;
    }

    eprintln!("[acp] no Rua server found at {base_url}; starting a standalone embedded runtime");
    run_embedded_acp(paths, &agent_id, selected_model.as_ref()).await
}
