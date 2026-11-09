pub fn expand_tilde(input: &str) -> String {
    if input == "~" {
        return rua_config::dirs::home_dir().to_string_lossy().into_owned();
    }
    if let Some(rest) = input
        .strip_prefix("~/")
        .or_else(|| input.strip_prefix("~\\"))
    {
        return rua_config::dirs::home_dir()
            .join(rest)
            .to_string_lossy()
            .into_owned();
    }
    input.to_string()
}
