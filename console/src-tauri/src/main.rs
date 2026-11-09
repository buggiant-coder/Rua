#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--verify-desktop-bundle") {
        let result = std::env::current_exe()
            .map_err(|e| e.to_string())
            .and_then(|path| rua_desktop::bundle::verify_current(&path));
        match result {
            Ok(receipt) => println!("{receipt}"),
            Err(error) => {
                eprintln!("{error}");
                std::process::exit(1);
            }
        }
        return;
    }
    rua_desktop::run();
}
