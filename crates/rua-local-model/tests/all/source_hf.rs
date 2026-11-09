use rua_local_model::source_hf::HfFileEntry;
use rua_local_model::HfSource;

#[test]
fn parse_hf_file_entry_uses_rfilename() {
    let json = r#"{"rfilename":"model-q4.gguf","size":4096}"#;

    let entry: HfFileEntry = serde_json::from_str(json).unwrap();

    assert_eq!(entry.filename, "model-q4.gguf");
    assert_eq!(entry.size, Some(4096));
}

#[test]
fn hf_file_filter_keeps_only_gguf_files() {
    let entries = vec![
        HfFileEntry {
            filename: "README.md".into(),
            size: Some(100),
        },
        HfFileEntry {
            filename: "model-q4_K_M.gguf".into(),
            size: Some(4_000_000),
        },
        HfFileEntry {
            filename: "model-q8.gguf".into(),
            size: Some(8_000_000),
        },
        HfFileEntry {
            filename: "config.json".into(),
            size: Some(50),
        },
    ];

    let gguf: Vec<_> = entries
        .into_iter()
        .filter(|file| file.filename.ends_with(".gguf"))
        .collect();

    assert_eq!(gguf.len(), 2);
    assert_eq!(gguf[0].filename, "model-q4_K_M.gguf");
}

#[test]
fn hf_download_url_uses_resolve_main_path() {
    let src = HfSource::new("lmstudio-community/Meta-Llama-3.1-8B-GGUF", None);
    let url = src.download_url("model-q4.gguf");

    assert_eq!(
        url,
        "https://huggingface.co/lmstudio-community/Meta-Llama-3.1-8B-GGUF/resolve/main/model-q4.gguf"
    );
}
