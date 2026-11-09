pub fn which_exists(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                command_file_names(cmd)
                    .into_iter()
                    .any(|name| dir.join(name).is_file())
            })
        })
        .unwrap_or(false)
}

fn command_file_names(cmd: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        let mut names = vec![cmd.to_string()];
        if std::path::Path::new(cmd).extension().is_none() {
            let pathext = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
            for ext in pathext.split(';').filter(|ext| !ext.trim().is_empty()) {
                names.push(format!("{cmd}{ext}"));
            }
        }
        names
    }
    #[cfg(not(windows))]
    {
        vec![cmd.to_string()]
    }
}
