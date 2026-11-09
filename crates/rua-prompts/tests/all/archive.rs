use rua_prompts::{archive, PromptLibrary};
use std::io::Write;
use tempfile::TempDir;

#[tokio::test]
async fn download_then_upload_round_trip_through_library() {
    let src_tmp = TempDir::new().expect("src tempdir");
    let src = PromptLibrary::new(src_tmp.path().to_path_buf());
    src.update_file("alpha", "alpha-content")
        .await
        .expect("write alpha");
    src.create_subdir_file("group", Some("beta"), "")
        .await
        .expect("create subdir beta");
    src.write_subdir_file("group", "beta", "beta-content")
        .await
        .expect("write beta");

    let zip_bytes = src.download().await.expect("download archive");
    assert!(!zip_bytes.is_empty(), "zip must not be empty");

    assert_eq!(&zip_bytes[..2], b"PK", "must start with zip magic bytes");

    let dst_tmp = TempDir::new().expect("dst tempdir");
    let dst = PromptLibrary::new(dst_tmp.path().to_path_buf());
    let msg = dst.upload(&zip_bytes).await.expect("upload accepted");
    assert_eq!(msg, "Extracted zip");

    let alpha = dst
        .get_file("alpha")
        .await
        .expect("get alpha")
        .expect("alpha present");
    assert_eq!(alpha.content, "alpha-content");

    let beta = dst
        .get_subdir_file("group", "beta")
        .await
        .expect("get beta")
        .expect("beta present");
    assert_eq!(beta.content, "beta-content");
}

#[tokio::test]
async fn upload_raw_md_falls_back_to_uploaded_md() {
    let dst_tmp = TempDir::new().expect("dst tempdir");
    let lib = PromptLibrary::new(dst_tmp.path().to_path_buf());
    let msg = lib
        .upload(b"# raw markdown body")
        .await
        .expect("upload raw md");
    assert_eq!(msg, "Uploaded file");

    let uploaded = lib
        .get_file("uploaded")
        .await
        .expect("get uploaded")
        .expect("present");
    assert_eq!(uploaded.content, "# raw markdown body");
}

#[tokio::test]
async fn download_returns_not_found_when_lib_dir_missing() {
    let tmp = TempDir::new().expect("tempdir");

    let missing = tmp.path().join("does-not-exist");
    let err = archive::download(&missing).expect_err("must fail");
    let msg = format!("{err}");
    assert!(msg.to_lowercase().contains("not found"), "msg: {msg}");
}

#[tokio::test]
async fn upload_rejects_zip_with_path_traversal() {
    let mut buf = Vec::<u8>::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("../escape.md", options)
            .expect("start escape entry");
        zip.write_all(b"bad").expect("write");
        zip.finish().expect("finish");
    }

    let dst_tmp = TempDir::new().expect("dst tempdir");
    let lib_dir = dst_tmp.path().join("prompt_library");
    std::fs::create_dir_all(&lib_dir).expect("mkdir lib dir");

    let err = PromptLibrary::new(dst_tmp.path().into())
        .upload(&buf)
        .await
        .expect_err("must reject traversal");
    let msg = format!("{err}");
    assert!(
        msg.to_lowercase().contains("unsafe path"),
        "expected unsafe path message, got: {msg}"
    );
}
