use rua_core::runtime_authority::browser::discover_chromium;
use std::path::{Path, PathBuf};

#[test]
fn discovers_system_chromium_without_considering_other_engines() {
    let chrome = Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");
    let found = discover_chromium("macos", Some(Path::new("/Users/test")), &[], |p| {
        p == chrome
    });
    assert_eq!(found.path.as_deref(), Some(chrome));
    let unavailable = discover_chromium("macos", None, &[], |p| {
        p == Path::new("/Applications/Safari.app/Contents/MacOS/Safari")
    });
    assert!(!unavailable.available);
    assert!(unavailable.diagnostics[0].contains("Chromium"));
}

#[test]
fn windows_discovery_accepts_spaces_and_unicode_install_roots() {
    let root = PathBuf::from("C:/Users/测试 用户/AppData/Local");
    let edge = root.join("Microsoft/Edge/Application/msedge.exe");
    let result = discover_chromium("windows", None, &[root], |path| path == edge);
    assert_eq!(result.path, Some(edge));
    assert!(!discover_chromium("android", None, &[], |_| true).available);
}
