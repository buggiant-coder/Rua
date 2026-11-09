use rua_session::display_media::{managed_image_refs, DisplayMediaArchive};

#[test]
fn native_image_bytes_use_existing_managed_media_layout() {
    let temp = tempfile::tempdir().unwrap();
    let media = DisplayMediaArchive::store_image_bytes(
        temp.path(),
        b"jpeg-bytes",
        "image/jpeg",
        "device-a",
        "phone-camera",
    )
    .unwrap();
    assert!(media.media_ref.starts_with("media/"));
    assert_eq!(
        std::fs::read(temp.path().join(&media.media_ref)).unwrap(),
        b"jpeg-bytes"
    );
    assert_eq!(media.origin.device_id, "device-a");
    assert!(media.origin.base_dir.is_none());
    assert!(DisplayMediaArchive::store_image_bytes(
        temp.path(),
        b"",
        "image/jpeg",
        "device-a",
        "phone-camera"
    )
    .is_err());
    assert!(DisplayMediaArchive::store_image_bytes(
        temp.path(),
        b"text",
        "text/plain",
        "device-a",
        "phone-camera"
    )
    .is_err());
}

#[test]
fn archives_local_markdown_and_figure_preserving_origin_and_identity() {
    let source = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let path = source.path().join("preview.png");
    std::fs::write(&path, b"png bytes").unwrap();
    let text = "![预览](preview.png)\n\n@figure\n  - src=preview.png caption=测试\n@end";
    let mut archive = DisplayMediaArchive::default();
    let (rewritten, media) = archive
        .rewrite(text, workspace.path(), source.path(), "desktop", |p| {
            source.path().join(p).canonicalize()
        })
        .unwrap();
    assert_eq!(media.len(), 1);
    assert_eq!(rewritten.matches(&media[0].media_ref).count(), 2);
    assert_eq!(media[0].origin.src, "preview.png");
    assert_eq!(media[0].origin.device_id, "desktop");
    assert_eq!(media[0].origin.base_dir.as_deref(), Some(source.path()));
    assert_eq!(
        std::fs::read(workspace.path().join(&media[0].media_ref)).unwrap(),
        b"png bytes"
    );
    std::fs::remove_file(path).unwrap();
    let (again, _) = archive
        .rewrite(text, workspace.path(), source.path(), "desktop", |_| {
            panic!("cached import must not reread source")
        })
        .unwrap();
    assert_eq!(rewritten, again);
    assert_eq!(managed_image_refs(&rewritten).len(), 2);
}

#[test]
fn ignores_examples_links_and_incomplete_figures() {
    let text = "`![x](missing.png)`\n\n```text\n@figure\n- src=missing.png\n@end\n```\n\n[link](missing.png)\n\n@figure\n- src=missing.png";
    let root = tempfile::tempdir().unwrap();
    let (rewritten, media) = DisplayMediaArchive::default()
        .rewrite(text, root.path(), root.path(), "desktop", |_| {
            panic!("not an image display")
        })
        .unwrap();
    assert_eq!(text, rewritten);
    assert!(media.is_empty());
}

#[test]
fn imports_reference_images_and_quoted_unicode_figure_paths() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("图片 a.png"), b"png").unwrap();
    let text = "![alt][preview]\n\n[preview]: <图片 a.png>\n\n@figure\n- caption=图 src=\"图片 a.png\"\n@end";
    let (rewritten, media) = DisplayMediaArchive::default()
        .rewrite(text, root.path(), root.path(), "desktop", |p| {
            root.path().join(p).canonicalize()
        })
        .unwrap();
    assert_eq!(media.len(), 1);
    assert_eq!(managed_image_refs(&rewritten).len(), 2);
}

#[test]
fn failed_import_does_not_return_local_reference_as_success() {
    let root = tempfile::tempdir().unwrap();
    let error = DisplayMediaArchive::default()
        .rewrite(
            "![x](/tmp/private.png)",
            root.path(),
            root.path(),
            "desktop",
            |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "denied",
                ))
            },
        )
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn canonical_references_do_not_resolve_original_paths() {
    let root = tempfile::tempdir().unwrap();
    let text = "![x](media/00000000-0000-4000-8000-000000000001)";
    let (rewritten, imported) = DisplayMediaArchive::default()
        .rewrite(text, root.path(), root.path(), "phone", |_| {
            panic!("canonical refs must not resolve local paths")
        })
        .unwrap();
    assert_eq!(text, rewritten);
    assert!(imported.is_empty());
}

#[test]
fn markdown_syntax_inside_a_figure_caption_is_not_a_second_image() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("image.png"), b"png").unwrap();
    let text = "@figure\n- src=image.png caption=\"![example](missing.png)\"\n@end";
    let (rewritten, imported) = DisplayMediaArchive::default()
        .rewrite(text, root.path(), root.path(), "desktop", |p| {
            root.path().join(p).canonicalize()
        })
        .unwrap();
    assert_eq!(imported.len(), 1);
    assert!(rewritten.contains("![example](missing.png)"));
}
