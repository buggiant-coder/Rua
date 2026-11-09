use axum::http::{Method, StatusCode};
use serde_json::json;

use super::support::{build_app, create_agent, delete_agent, read_json, request};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_config_roundtrips_for_header_and_flat_agent_scoped_routes() {
    let app = build_app();
    let root_agent = "integ_cfg_running_root";
    let scoped_agent = "integ_cfg_running_scoped";
    create_agent(&app.app, root_agent, "Running config root agent").await;
    create_agent(&app.app, scoped_agent, "Running config scoped agent").await;

    let root_before = request(
        &app.app,
        Method::GET,
        "/api/agent/running-config",
        None,
        Some(root_agent),
    )
    .await;
    assert_eq!(root_before.status(), StatusCode::OK);
    let mut root_body = read_json(root_before).await;
    let root_max = root_body["max_iters"].as_u64().expect("max_iters") as u32;
    root_body["max_iters"] = json!(root_max + 1);

    let root_put = request(
        &app.app,
        Method::PUT,
        "/api/agent/running-config",
        Some(root_body.clone()),
        Some(root_agent),
    )
    .await;
    assert_eq!(root_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(root_put).await["max_iters"],
        root_body["max_iters"]
    );

    let root_after = request(
        &app.app,
        Method::GET,
        "/api/agent/running-config",
        None,
        Some(root_agent),
    )
    .await;
    assert_eq!(
        read_json(root_after).await["max_iters"],
        root_body["max_iters"]
    );

    let scoped_path = format!("/api/agents/{scoped_agent}/running-config");
    let scoped_before = request(&app.app, Method::GET, &scoped_path, None, None).await;
    assert_eq!(scoped_before.status(), StatusCode::OK);
    let mut scoped_body = read_json(scoped_before).await;
    scoped_body["max_iters"] = json!(0);

    let scoped_put = request(
        &app.app,
        Method::PUT,
        &scoped_path,
        Some(scoped_body.clone()),
        None,
    )
    .await;
    assert_eq!(scoped_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(scoped_put).await["max_iters"],
        scoped_body["max_iters"]
    );

    let scoped_after = request(&app.app, Method::GET, &scoped_path, None, None).await;
    assert_eq!(
        read_json(scoped_after).await["max_iters"],
        scoped_body["max_iters"]
    );

    delete_agent(&app.app, root_agent).await;
    delete_agent(&app.app, scoped_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_config_isolated_between_header_and_path_scoped_agents() {
    let app = build_app();
    let header_agent = "integ_cfg_running_header_iso";
    let path_agent = "integ_cfg_running_path_iso";
    create_agent(&app.app, header_agent, "Running config header isolation").await;
    create_agent(&app.app, path_agent, "Running config path isolation").await;

    let mut header_body = read_json(
        request(
            &app.app,
            Method::GET,
            "/api/agent/running-config",
            None,
            Some(header_agent),
        )
        .await,
    )
    .await;
    header_body["max_iters"] = json!(111);
    header_body["qwenpaw_alignment_marker"] = json!("header-agent");

    let header_put = request(
        &app.app,
        Method::PUT,
        "/api/agent/running-config",
        Some(header_body.clone()),
        Some(header_agent),
    )
    .await;
    assert_eq!(header_put.status(), StatusCode::OK);
    assert_eq!(read_json(header_put).await, header_body);

    let path = format!("/api/agents/{path_agent}/running-config");
    let mut path_body = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    path_body["max_iters"] = json!(222);
    path_body["qwenpaw_alignment_marker"] = json!("path-agent");

    let path_put = request(&app.app, Method::PUT, &path, Some(path_body.clone()), None).await;
    assert_eq!(path_put.status(), StatusCode::OK);
    assert_eq!(read_json(path_put).await, path_body);

    let header_via_root = read_json(
        request(
            &app.app,
            Method::GET,
            "/api/agent/running-config",
            None,
            Some(header_agent),
        )
        .await,
    )
    .await;
    assert_eq!(header_via_root["max_iters"], 111);
    assert_eq!(header_via_root["qwenpaw_alignment_marker"], "header-agent");

    let header_via_path = read_json(
        request(
            &app.app,
            Method::GET,
            &format!("/api/agents/{header_agent}/running-config"),
            None,
            None,
        )
        .await,
    )
    .await;
    assert_eq!(header_via_path, header_via_root);

    let path_via_header = read_json(
        request(
            &app.app,
            Method::GET,
            "/api/agent/running-config",
            None,
            Some(path_agent),
        )
        .await,
    )
    .await;
    assert_eq!(path_via_header["max_iters"], 222);
    assert_eq!(path_via_header["qwenpaw_alignment_marker"], "path-agent");

    let default_agent = read_json(
        request(
            &app.app,
            Method::GET,
            "/api/agent/running-config",
            None,
            Some("default"),
        )
        .await,
    )
    .await;
    assert_eq!(default_agent["max_iters"], 100);
    assert!(default_agent.get("qwenpaw_alignment_marker").is_none());

    delete_agent(&app.app, header_agent).await;
    delete_agent(&app.app, path_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_config_path_scope_wins_over_conflicting_header_and_preserves_nested_unknowns() {
    let app = build_app();
    let header_agent = "integ_cfg_running_conflict_header";
    let path_agent = "integ_cfg_running_conflict_path";
    create_agent(&app.app, header_agent, "Running config conflicting header").await;
    create_agent(&app.app, path_agent, "Running config path wins").await;

    let mut header_body = read_json(
        request(
            &app.app,
            Method::GET,
            "/api/agent/running-config",
            None,
            Some(header_agent),
        )
        .await,
    )
    .await;
    header_body["max_iters"] = json!(333);
    header_body["qwenpaw_alignment_marker"] = json!("header-scope");
    let header_put = request(
        &app.app,
        Method::PUT,
        "/api/agent/running-config",
        Some(header_body.clone()),
        Some(header_agent),
    )
    .await;
    assert_eq!(header_put.status(), StatusCode::OK);

    let path_route = format!("/api/agents/{path_agent}/running-config");
    let mut path_body =
        read_json(request(&app.app, Method::GET, &path_route, None, None).await).await;
    path_body["max_iters"] = json!(444);
    path_body["qwenpaw_alignment_marker"] = json!("path-scope");
    path_body["nested_unknown_contract"] = json!({
        "preserve": true,
        "items": ["alpha", "beta"],
        "limits": {
            "soft": 3,
            "hard": null
        }
    });

    let path_put_with_conflicting_header = request(
        &app.app,
        Method::PUT,
        &path_route,
        Some(path_body.clone()),
        Some(header_agent),
    )
    .await;
    assert_eq!(path_put_with_conflicting_header.status(), StatusCode::OK);
    assert_eq!(read_json(path_put_with_conflicting_header).await, path_body);

    let header_after = read_json(
        request(
            &app.app,
            Method::GET,
            "/api/agent/running-config",
            None,
            Some(header_agent),
        )
        .await,
    )
    .await;
    assert_eq!(header_after["max_iters"], 333);
    assert_eq!(header_after["qwenpaw_alignment_marker"], "header-scope");
    assert!(header_after.get("nested_unknown_contract").is_none());

    let path_after = read_json(request(&app.app, Method::GET, &path_route, None, None).await).await;
    assert_eq!(path_after["max_iters"], 444);
    assert_eq!(path_after["qwenpaw_alignment_marker"], "path-scope");
    assert_eq!(
        path_after["nested_unknown_contract"],
        path_body["nested_unknown_contract"]
    );

    delete_agent(&app.app, header_agent).await;
    delete_agent(&app.app, path_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_config_context_compact_fields_roundtrip_with_rust_shape() {
    let app = build_app();
    let agent_id = "integ_cfg_running_context";
    create_agent(&app.app, agent_id, "Running context compact").await;
    let path = format!("/api/agents/{agent_id}/running-config");

    let before = request(&app.app, Method::GET, &path, None, None).await;
    assert_eq!(before.status(), StatusCode::OK);
    let mut body = read_json(before).await;
    assert!(
        body["context_compact"].is_object(),
        "Rust running config should expose context_compact, got {body:?}"
    );

    body["context_compact"]["blocking_compact_ratio"] = json!(0.5);
    body["context_compact"]["context_reserve_ratio"] = json!(0.2);
    body["context_compact"]["history_replacement_enabled"] = json!(true);
    body["tool_result_compact"]["projection_truncation_enabled"] = json!(true);
    body["tool_result_compact"]["projection_truncation_batch_size"] = json!(10);
    body["context_compact"]["alignment_nested_unknown"] = json!({
        "preserved": true,
        "source": "qwenpaw-memory-context"
    });

    let put = request(&app.app, Method::PUT, &path, Some(body.clone()), None).await;
    assert_eq!(put.status(), StatusCode::OK);
    let put_body = read_json(put).await;
    assert_eq!(put_body["context_compact"], body["context_compact"]);
    assert_eq!(put_body["tool_result_compact"], body["tool_result_compact"]);

    let after = request(&app.app, Method::GET, &path, None, None).await;
    assert_eq!(after.status(), StatusCode::OK);
    let after_body = read_json(after).await;
    assert_eq!(
        after_body["context_compact"]["blocking_compact_ratio"],
        json!(0.5)
    );
    assert_eq!(
        after_body["context_compact"]["context_reserve_ratio"],
        json!(0.2)
    );
    assert_eq!(
        after_body["context_compact"]["history_replacement_enabled"],
        json!(true)
    );
    assert_eq!(
        after_body["tool_result_compact"]["projection_truncation_enabled"],
        json!(true)
    );
    assert_eq!(
        after_body["tool_result_compact"]["projection_truncation_batch_size"],
        10
    );
    let mut invalid = after_body.clone();
    invalid["tool_result_compact"]["projection_truncation_batch_size"] = json!(0);
    let rejected = request(&app.app, Method::PUT, &path, Some(invalid), None).await;
    assert_eq!(rejected.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(read_json(rejected).await["detail"]
        .as_str()
        .unwrap()
        .contains("projection_truncation_batch_size"));
    assert_eq!(
        read_json(request(&app.app, Method::GET, &path, None, None).await).await,
        after_body
    );
    assert_eq!(
        after_body["context_compact"]["alignment_nested_unknown"],
        body["context_compact"]["alignment_nested_unknown"]
    );
    assert!(
        after_body.get("light_context_config").is_none(),
        "Rust should not grow QwenPaw's light_context_config alias without a route decision"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_config_reset_restores_application_defaults_and_exposes_chat_defaults() {
    let app = build_app();
    let agent_id = "integ_cfg_running_reset";
    create_agent(&app.app, agent_id, "Running config reset").await;
    let path = format!("/api/agents/{agent_id}/running-config");

    let mut configured = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    configured["max_iters"] = json!(17);
    configured["show_debug_menu"] = json!(true);
    configured["context_compact"]["blocking_compact_ratio"] = json!(0.7);
    configured["llm_max_retries"] = json!(4);
    let updated = request(&app.app, Method::PUT, &path, Some(configured.clone()), None).await;
    assert_eq!(updated.status(), StatusCode::OK);
    assert_eq!(read_json(updated).await["show_debug_menu"], true);

    let chat_defaults = request(
        &app.app,
        Method::GET,
        &format!("{path}/chat-defaults"),
        None,
        None,
    )
    .await;
    assert_eq!(chat_defaults.status(), StatusCode::OK);
    let chat_defaults = read_json(chat_defaults).await;
    assert_eq!(chat_defaults["max_iters"], 17);
    assert_eq!(
        chat_defaults["context_compact"]["blocking_compact_ratio"],
        json!(0.7)
    );
    assert_eq!(chat_defaults["llm_max_retries"], 4);
    assert_eq!(chat_defaults["show_debug_menu"], true);

    let reset = request(&app.app, Method::DELETE, &path, None, None).await;
    assert_eq!(reset.status(), StatusCode::OK);
    let reset = read_json(reset).await;
    assert_eq!(reset["max_iters"], 100);
    assert_eq!(
        reset["context_compact"]["blocking_compact_ratio"],
        json!(0.9)
    );
    assert_eq!(reset["show_debug_menu"], false);

    let after = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    assert_eq!(after, reset);

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_config_get_defaults_missing_context_compact_fields_from_old_agent_json() {
    let app = build_app();
    let agent_id = "integ_cfg_running_context_defaults";
    create_agent(&app.app, agent_id, "Running context compact defaults").await;
    let agent_json_path = app
        .data_dir
        .join("workspaces")
        .join(agent_id)
        .join("agent.json");
    let mut agent_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&agent_json_path).expect("agent json"))
            .expect("agent json value");
    agent_json["running"]["context_compact"] = json!({
        "context_compact_enabled": true,
        "context_reserve_ratio": 0.25,
        "legacy_context_unknown": "preserve-me"
    });
    std::fs::write(
        &agent_json_path,
        serde_json::to_vec_pretty(&agent_json).expect("agent json bytes"),
    )
    .expect("write old agent json");

    let path = format!("/api/agents/{agent_id}/running-config");
    let resp = request(&app.app, Method::GET, &path, None, None).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = read_json(resp).await;
    assert_eq!(
        body["context_compact"]["blocking_compact_ratio"],
        json!(0.9)
    );
    assert_eq!(
        body["context_compact"]["early_compaction_target_ratio"],
        json!(0.5)
    );
    assert_eq!(
        body["context_compact"]["context_reserve_ratio"],
        json!(0.25)
    );
    assert_eq!(
        body["context_compact"]["legacy_context_unknown"],
        json!("preserve-me")
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn running_config_rejects_invalid_llm_retry_and_rate_limit_values_without_mutation() {
    let app = build_app();
    let agent_id = "integ_cfg_running_validation";
    create_agent(&app.app, agent_id, "Running config validation").await;
    let path = format!("/api/agents/{agent_id}/running-config");

    let mut valid = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
    valid["llm_retry_enabled"] = json!(true);
    valid["llm_max_retries"] = json!(2);
    valid["llm_backoff_base"] = json!(0.5);
    valid["llm_backoff_cap"] = json!(5.0);
    valid["llm_max_concurrent"] = json!(2);
    valid["llm_rate_limit_pause"] = json!(5.0);
    valid["llm_rate_limit_jitter"] = json!(0.0);
    valid["llm_acquire_timeout"] = json!(10.0);

    let seed = request(&app.app, Method::PUT, &path, Some(valid.clone()), None).await;
    assert_eq!(seed.status(), StatusCode::OK);

    for (field, bad_value) in [
        ("llm_max_retries", json!(0)),
        ("llm_backoff_base", json!(0.01)),
        ("llm_backoff_cap", json!(0.1)),
        ("llm_max_concurrent", json!(0)),
        ("llm_rate_limit_pause", json!(0.1)),
        ("llm_rate_limit_jitter", json!(-1.0)),
        ("llm_acquire_timeout", json!(1.0)),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = bad_value;

        let resp = request(&app.app, Method::PUT, &path, Some(invalid), None).await;
        assert_eq!(
            resp.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{field} should be rejected before persistence"
        );
        let body = read_json(resp).await;
        assert!(
            body["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains(field)),
            "validation detail should name {field}, got {body:?}"
        );

        let after = read_json(request(&app.app, Method::GET, &path, None, None).await).await;
        assert_eq!(
            after, valid,
            "invalid {field} update must not mutate the running config"
        );
    }

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flat_agent_settings_roundtrip_language_audio_transcription_and_prompt_files() {
    let app = build_app();
    let agent_id = "integ_cfg_settings";
    create_agent(&app.app, agent_id, "Settings agent").await;

    let language_path = format!("/api/agents/{agent_id}/language");
    let language_put = request(
        &app.app,
        Method::PUT,
        &language_path,
        Some(json!({"language": "en"})),
        None,
    )
    .await;
    assert_eq!(language_put.status(), StatusCode::OK);
    assert_eq!(read_json(language_put).await["language"], "en");
    let language_get = request(&app.app, Method::GET, &language_path, None, None).await;
    assert_eq!(read_json(language_get).await["language"], "en");

    let audio_path = format!("/api/agents/{agent_id}/audio-mode");
    let audio_put = request(
        &app.app,
        Method::PUT,
        &audio_path,
        Some(json!({"audio_mode": "native"})),
        None,
    )
    .await;
    assert_eq!(audio_put.status(), StatusCode::OK);
    assert_eq!(read_json(audio_put).await["audio_mode"], "native");
    let audio_get = request(&app.app, Method::GET, &audio_path, None, None).await;
    assert_eq!(read_json(audio_get).await["audio_mode"], "native");

    let transcription_path = format!("/api/agents/{agent_id}/transcription-provider-type");
    let transcription_put = request(
        &app.app,
        Method::PUT,
        &transcription_path,
        Some(json!({"transcription_provider_type": "whisper_api"})),
        None,
    )
    .await;
    assert_eq!(transcription_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(transcription_put).await["transcription_provider_type"],
        "whisper_api"
    );
    let transcription_get = request(&app.app, Method::GET, &transcription_path, None, None).await;
    assert_eq!(
        read_json(transcription_get).await["transcription_provider_type"],
        "whisper_api"
    );

    let prompt_path = format!("/api/agents/{agent_id}/system-prompt-files");
    let prompt_put = request(
        &app.app,
        Method::PUT,
        &prompt_path,
        Some(json!(["AGENTS.md", "PROFILE.md"])),
        None,
    )
    .await;
    assert_eq!(prompt_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(prompt_put).await,
        json!(["AGENTS.md", "PROFILE.md"])
    );
    let prompt_get = request(&app.app, Method::GET, &prompt_path, None, None).await;
    assert_eq!(
        read_json(prompt_get).await,
        json!(["AGENTS.md", "PROFILE.md"])
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flat_root_agent_settings_use_header_selected_agent_context() {
    let app = build_app();
    let header_agent = "integ_cfg_settings_header";
    let other_agent = "integ_cfg_settings_header_other";
    create_agent(&app.app, header_agent, "Settings header agent").await;
    create_agent(&app.app, other_agent, "Settings other agent").await;

    let language_put = request(
        &app.app,
        Method::PUT,
        "/api/agent/language",
        Some(json!({"language": "en"})),
        Some(header_agent),
    )
    .await;
    assert_eq!(language_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(language_put).await,
        json!({"agent_id": header_agent, "language": "en"})
    );

    let language_get = request(
        &app.app,
        Method::GET,
        "/api/agent/language",
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(
        read_json(language_get).await,
        json!({"agent_id": header_agent, "language": "en"})
    );

    let other_language = request(
        &app.app,
        Method::GET,
        "/api/agent/language",
        None,
        Some(other_agent),
    )
    .await;
    assert_eq!(
        read_json(other_language).await,
        json!({"agent_id": other_agent, "language": "zh"}),
        "root /api/agent/language must not leak the header-selected agent's value"
    );

    let prompt_put = request(
        &app.app,
        Method::PUT,
        "/api/agent/system-prompt-files",
        Some(json!(["HEADER.md", "SHARED.md"])),
        Some(header_agent),
    )
    .await;
    assert_eq!(prompt_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(prompt_put).await,
        json!(["HEADER.md", "SHARED.md"])
    );

    let prompt_get = request(
        &app.app,
        Method::GET,
        "/api/agent/system-prompt-files",
        None,
        Some(header_agent),
    )
    .await;
    assert_eq!(
        read_json(prompt_get).await,
        json!(["HEADER.md", "SHARED.md"])
    );

    let other_prompt = request(
        &app.app,
        Method::GET,
        "/api/agent/system-prompt-files",
        None,
        Some(other_agent),
    )
    .await;
    assert_ne!(
        read_json(other_prompt).await,
        json!(["HEADER.md", "SHARED.md"]),
        "root /api/agent/system-prompt-files must use X-Agent-Id instead of global state"
    );

    delete_agent(&app.app, header_agent).await;
    delete_agent(&app.app, other_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn flat_agent_settings_keep_agent_owned_fields_isolated() {
    let app = build_app();
    let left_agent = "integ_cfg_settings_left_iso";
    let right_agent = "integ_cfg_settings_right_iso";
    create_agent(&app.app, left_agent, "Settings left isolation").await;
    create_agent(&app.app, right_agent, "Settings right isolation").await;

    let left_language = format!("/api/agents/{left_agent}/language");
    let right_language = format!("/api/agents/{right_agent}/language");
    let left_lang_put = request(
        &app.app,
        Method::PUT,
        &left_language,
        Some(json!({"language": "en"})),
        None,
    )
    .await;
    assert_eq!(left_lang_put.status(), StatusCode::OK);
    let right_lang_put = request(
        &app.app,
        Method::PUT,
        &right_language,
        Some(json!({"language": "ru"})),
        None,
    )
    .await;
    assert_eq!(right_lang_put.status(), StatusCode::OK);

    assert_eq!(
        read_json(request(&app.app, Method::GET, &left_language, None, None).await).await
            ["language"],
        "en"
    );
    assert_eq!(
        read_json(request(&app.app, Method::GET, &right_language, None, None).await).await
            ["language"],
        "ru"
    );

    let left_prompt = format!("/api/agents/{left_agent}/system-prompt-files");
    let right_prompt = format!("/api/agents/{right_agent}/system-prompt-files");
    let left_prompt_body = json!(["AGENTS.md", "LEFT.md"]);
    let right_prompt_body = json!(["RIGHT.md"]);

    let left_prompt_put = request(
        &app.app,
        Method::PUT,
        &left_prompt,
        Some(left_prompt_body.clone()),
        None,
    )
    .await;
    assert_eq!(left_prompt_put.status(), StatusCode::OK);
    let right_prompt_put = request(
        &app.app,
        Method::PUT,
        &right_prompt,
        Some(right_prompt_body.clone()),
        None,
    )
    .await;
    assert_eq!(right_prompt_put.status(), StatusCode::OK);

    assert_eq!(
        read_json(request(&app.app, Method::GET, &left_prompt, None, None).await).await,
        left_prompt_body
    );
    assert_eq!(
        read_json(request(&app.app, Method::GET, &right_prompt, None, None).await).await,
        right_prompt_body
    );

    delete_agent(&app.app, left_agent).await;
    delete_agent(&app.app, right_agent).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn agent_language_rejects_invalid_or_missing_values_without_mutating_existing_agent() {
    let app = build_app();
    let agent_id = "integ_cfg_settings_language_reject";
    create_agent(&app.app, agent_id, "Settings language rejection").await;

    let language_path = format!("/api/agents/{agent_id}/language");
    let valid_put = request(
        &app.app,
        Method::PUT,
        &language_path,
        Some(json!({"language": "EN"})),
        None,
    )
    .await;
    assert_eq!(valid_put.status(), StatusCode::OK);
    assert_eq!(read_json(valid_put).await["language"], "en");

    for body in [
        json!({"language": "xx"}),
        json!({"language": "pt-BR"}),
        json!({"language": "id"}),
        json!({"language": "vi"}),
        json!({"language": ""}),
        json!({"language": "   "}),
        json!({}),
        json!({"language": 42}),
    ] {
        let resp = request(&app.app, Method::PUT, &language_path, Some(body), None).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let detail = read_json(resp).await["detail"]
            .as_str()
            .expect("detail")
            .to_string();
        assert!(
            detail.contains("Invalid language"),
            "unexpected detail: {detail}"
        );

        let after = request(&app.app, Method::GET, &language_path, None, None).await;
        assert_eq!(read_json(after).await["language"], "en");
    }

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn system_prompt_files_accept_empty_list_and_reject_non_array_payloads() {
    let app = build_app();
    let agent_id = "integ_cfg_prompt_files_boundary";
    create_agent(&app.app, agent_id, "System prompt files boundary").await;

    let prompt_path = format!("/api/agents/{agent_id}/system-prompt-files");
    let seed = request(
        &app.app,
        Method::PUT,
        &prompt_path,
        Some(json!(["AGENTS.md"])),
        None,
    )
    .await;
    assert_eq!(seed.status(), StatusCode::OK);
    assert_eq!(read_json(seed).await, json!(["AGENTS.md"]));

    let empty = request(&app.app, Method::PUT, &prompt_path, Some(json!([])), None).await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert_eq!(read_json(empty).await, json!([]));

    for body in [
        json!({"files": ["AGENTS.md"]}),
        json!("AGENTS.md"),
        json!([{"name": "AGENTS.md"}]),
    ] {
        let resp = request(&app.app, Method::PUT, &prompt_path, Some(body), None).await;
        assert!(
            resp.status().is_client_error(),
            "unexpected status: {}",
            resp.status()
        );
        let after = request(&app.app, Method::GET, &prompt_path, None, None).await;
        assert_eq!(read_json(after).await, json!([]));
    }

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn voice_settings_roundtrip_through_header_and_flat_agent_scoped_routes() {
    let app = build_app();
    let agent_id = "integ_cfg_voice_settings";
    create_agent(&app.app, agent_id, "Voice settings agent").await;

    let audio_root = request(&app.app, Method::GET, "/api/agent/audio-mode", None, None).await;
    assert_eq!(audio_root.status(), StatusCode::OK);
    assert_eq!(read_json(audio_root).await["audio_mode"], "auto");

    let scoped_audio_path = format!("/api/agents/{agent_id}/audio-mode");
    let audio_put = request(
        &app.app,
        Method::PUT,
        &scoped_audio_path,
        Some(json!({"audio_mode": "native"})),
        None,
    )
    .await;
    assert_eq!(audio_put.status(), StatusCode::OK);
    assert_eq!(read_json(audio_put).await["audio_mode"], "native");

    let audio_root_after =
        request(&app.app, Method::GET, "/api/agent/audio-mode", None, None).await;
    assert_eq!(read_json(audio_root_after).await["audio_mode"], "native");

    let transcription_root = request(
        &app.app,
        Method::GET,
        "/api/agent/transcription-provider-type",
        None,
        None,
    )
    .await;
    assert_eq!(transcription_root.status(), StatusCode::OK);
    assert_eq!(
        read_json(transcription_root).await["transcription_provider_type"],
        "disabled"
    );

    let scoped_transcription_path = format!("/api/agents/{agent_id}/transcription-provider-type");
    let transcription_put = request(
        &app.app,
        Method::PUT,
        &scoped_transcription_path,
        Some(json!({"transcription_provider_type": "whisper_api"})),
        None,
    )
    .await;
    assert_eq!(transcription_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(transcription_put).await["transcription_provider_type"],
        "whisper_api"
    );

    let transcription_root_after = request(
        &app.app,
        Method::GET,
        "/api/agent/transcription-provider-type",
        None,
        None,
    )
    .await;
    assert_eq!(
        read_json(transcription_root_after).await["transcription_provider_type"],
        "whisper_api"
    );

    delete_agent(&app.app, agent_id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scoped_voice_settings_are_global_rust_voice_configuration_not_agent_owned() {
    let app = build_app();
    let left_agent = "integ_cfg_voice_global_left";
    let right_agent = "integ_cfg_voice_global_right";
    create_agent(&app.app, left_agent, "Voice global left").await;
    create_agent(&app.app, right_agent, "Voice global right").await;

    let left_audio = format!("/api/agents/{left_agent}/audio-mode");
    let right_audio = format!("/api/agents/{right_agent}/audio-mode");
    let left_audio_put = request(
        &app.app,
        Method::PUT,
        &left_audio,
        Some(json!({"audio_mode": "native"})),
        None,
    )
    .await;
    assert_eq!(left_audio_put.status(), StatusCode::OK);
    assert_eq!(read_json(left_audio_put).await["audio_mode"], "native");
    assert_eq!(
        read_json(request(&app.app, Method::GET, &right_audio, None, None).await).await
            ["audio_mode"],
        "native"
    );

    let right_audio_put = request(
        &app.app,
        Method::PUT,
        &right_audio,
        Some(json!({"audio_mode": "auto"})),
        None,
    )
    .await;
    assert_eq!(right_audio_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(request(&app.app, Method::GET, &left_audio, None, None).await).await
            ["audio_mode"],
        "auto"
    );

    let left_transcription = format!("/api/agents/{left_agent}/transcription-provider-type");
    let right_transcription = format!("/api/agents/{right_agent}/transcription-provider-type");
    let left_type_put = request(
        &app.app,
        Method::PUT,
        &left_transcription,
        Some(json!({"transcription_provider_type": "whisper_api"})),
        None,
    )
    .await;
    assert_eq!(left_type_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(left_type_put).await["transcription_provider_type"],
        "whisper_api"
    );
    assert_eq!(
        read_json(request(&app.app, Method::GET, &right_transcription, None, None).await).await
            ["transcription_provider_type"],
        "whisper_api"
    );

    let right_provider = format!("/api/agents/{right_agent}/transcription-provider");
    let right_provider_put = request(
        &app.app,
        Method::PUT,
        &right_provider,
        Some(json!({"provider_id": "speech-provider-1"})),
        None,
    )
    .await;
    assert_eq!(right_provider_put.status(), StatusCode::OK);
    assert_eq!(
        read_json(right_provider_put).await["provider_id"],
        "speech-provider-1"
    );

    let root_providers = request(
        &app.app,
        Method::GET,
        "/api/agent/transcription-providers",
        None,
        None,
    )
    .await;
    let root_body = read_json(root_providers).await;
    assert_eq!(root_body["providers"], json!([]));
    assert_eq!(root_body["configured_provider_id"], "speech-provider-1");

    let root_reset = request(
        &app.app,
        Method::PUT,
        "/api/agent/transcription-provider-type",
        Some(json!({"transcription_provider_type": "disabled"})),
        None,
    )
    .await;
    assert_eq!(root_reset.status(), StatusCode::OK);

    delete_agent(&app.app, left_agent).await;
    delete_agent(&app.app, right_agent).await;
}
