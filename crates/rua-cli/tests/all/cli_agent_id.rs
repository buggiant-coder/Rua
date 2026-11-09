use clap::Parser;
use rua_cli::channels::{
    build_channel_send_payload, channel_config_endpoint, channels_config_endpoint,
    messages_send_endpoint, ChannelsCommands,
};
use rua_cli::console::{chat_endpoint, chats_collection_endpoint, chats_list_endpoint};
use rua_cli::cron::{
    cron_job_endpoint, cron_job_run_endpoint, cron_job_state_endpoint, cron_jobs_endpoint,
};
use rua_cli::skills::{
    skill_disable_endpoint, skill_enable_endpoint, skill_endpoint, skill_save_endpoint,
    skills_list_endpoint,
};
use rua_cli::{Cli, Commands};
use serde_json::json;

#[test]
fn agent_id_defaults_to_default_for_api_subcommands() {
    let cli = Cli::try_parse_from(["rua", "cron", "list"]).expect("cron list parses");

    assert_eq!(cli.agent_id, "default");
    assert!(matches!(cli.command, Commands::Cron(_)));
}

#[test]
fn agent_id_global_flag_parses_before_subcommand() {
    let cli = Cli::try_parse_from(["rua", "--agent-id", "abc123", "chats", "list"])
        .expect("global agent id before subcommand parses");

    assert_eq!(cli.agent_id, "abc123");
    assert!(matches!(cli.command, Commands::Chats(_)));
}

#[test]
fn agent_id_global_flag_parses_after_subcommand() {
    let cli = Cli::try_parse_from(["rua", "skills", "--agent-id", "team_bot", "list"])
        .expect("global agent id after subcommand parses");

    assert_eq!(cli.agent_id, "team_bot");
    assert!(matches!(cli.command, Commands::Skills(_)));
}

#[test]
fn agent_id_global_flag_parses_for_channels_subcommands() {
    let cli = Cli::try_parse_from([
        "rua",
        "channels",
        "--agent-id",
        "team_bot",
        "send",
        "--channel",
        "console",
        "--target-user",
        "alice",
        "--target-session",
        "sid-1",
        "--text",
        "hello",
    ])
    .expect("channels send should accept global agent id");

    assert_eq!(cli.agent_id, "team_bot");
    let Commands::Channels(args) = cli.command else {
        panic!("expected channels command");
    };
    let ChannelsCommands::Send {
        sender_agent_id, ..
    } = args.command
    else {
        panic!("expected channels send command");
    };
    assert_eq!(
        sender_agent_id, None,
        "channels send should default sender to global --agent-id"
    );
}

#[test]
fn channels_send_keeps_command_specific_agent_override() {
    let cli = Cli::try_parse_from([
        "rua",
        "--agent-id",
        "team_bot",
        "channels",
        "send",
        "--sender-agent-id",
        "sender_bot",
        "--channel",
        "console",
        "--target-user",
        "alice",
        "--target-session",
        "sid-1",
        "--text",
        "hello",
    ])
    .expect("channels send should accept explicit sender override");

    assert_eq!(cli.agent_id, "team_bot");
    let Commands::Channels(args) = cli.command else {
        panic!("expected channels command");
    };
    let ChannelsCommands::Send {
        sender_agent_id, ..
    } = args.command
    else {
        panic!("expected channels send command");
    };
    assert_eq!(sender_agent_id.as_deref(), Some("sender_bot"));
}

#[test]
fn chats_endpoints_are_path_scoped_by_global_agent_id() {
    assert_eq!(
        chats_collection_endpoint("team_bot"),
        "/api/agents/team_bot/chats"
    );
    assert_eq!(
        chats_list_endpoint("team_bot", Some("alice"), Some("console")),
        "/api/agents/team_bot/chats?user_id=alice&channel=console"
    );
    assert_eq!(
        chat_endpoint("team_bot", "chat-1"),
        "/api/agents/team_bot/chats/chat-1"
    );
}

#[test]
fn cron_endpoints_stay_root_scoped_and_use_api_client_agent_header() {
    assert_eq!(cron_jobs_endpoint(), "/api/cron/jobs");
    assert_eq!(cron_job_endpoint("job-1"), "/api/cron/jobs/job-1");
    assert_eq!(cron_job_run_endpoint("job-1"), "/api/cron/jobs/job-1/run");
    assert_eq!(
        cron_job_state_endpoint("job-1"),
        "/api/cron/jobs/job-1/state"
    );
}

#[test]
fn skills_endpoints_use_explicit_encoded_agent_paths() {
    assert_eq!(
        skills_list_endpoint("agent/a"),
        "/api/agents/agent%2Fa/skills"
    );
    assert_eq!(
        skill_endpoint("agent/a", "calendar"),
        "/api/agents/agent%2Fa/skills/calendar"
    );
    assert_eq!(
        skill_enable_endpoint("agent/a", "calendar"),
        "/api/agents/agent%2Fa/skills/calendar/enable"
    );
    assert_eq!(
        skill_disable_endpoint("agent/a", "calendar"),
        "/api/agents/agent%2Fa/skills/calendar/disable"
    );
    assert_eq!(
        skill_save_endpoint("agent/a"),
        "/api/agents/agent%2Fa/skills/save"
    );
}

#[test]
fn channels_endpoints_and_send_payload_follow_rust_contract() {
    assert_eq!(channels_config_endpoint(), "/api/config/channels");
    assert_eq!(
        channel_config_endpoint("dingtalk"),
        "/api/config/channels/dingtalk"
    );
    assert_eq!(messages_send_endpoint(), "/api/messages/send");
    assert_eq!(
        build_channel_send_payload("console", "alice", "sid-1", "hello"),
        json!({
            "channel": "console",
            "target_user": "alice",
            "target_session": "sid-1",
            "text": "hello",
        })
    );
}
