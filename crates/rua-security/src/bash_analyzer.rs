use tree_sitter::{Node, Parser};

const SHELL_COMMANDS: &[&str] = &[
    "sh", "bash", "zsh", "dash", "ash", "ksh", "csh", "tcsh", "fish",
];

const NETWORK_LOADERS: &[&str] = &["curl", "wget", "fetch"];

#[derive(Debug, Clone, PartialEq)]
pub struct BashAnalysis {
    pub commands: Vec<Command>,
    pub has_pipe_to_shell: bool,
    pub has_command_substitution: bool,
    pub has_fork_bomb: bool,
    pub has_base64_to_shell: bool,
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Command {
    pub name: String,
    pub args: Vec<String>,
    pub path_args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Redirect {
    pub target: String,
}

pub struct BashAnalyzer;

impl Default for BashAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

impl BashAnalyzer {
    pub fn new() -> Self {
        Self
    }

    pub fn analyze(&self, command: &str) -> BashAnalysis {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .expect("tree-sitter-bash grammar");

        let tree = match parser.parse(command, None) {
            Some(t) => t,
            None => return BashAnalysis::empty(),
        };

        let source = command.as_bytes();
        let mut analysis = BashAnalysis::empty();

        analysis.has_fork_bomb = detect_fork_bomb(command);

        walk(tree.root_node(), source, &mut analysis);
        analysis
    }
}

impl BashAnalysis {
    fn empty() -> Self {
        Self {
            commands: Vec::new(),
            has_pipe_to_shell: false,
            has_command_substitution: false,
            has_fork_bomb: false,
            has_base64_to_shell: false,
            redirects: Vec::new(),
        }
    }
}

fn walk(node: Node, source: &[u8], analysis: &mut BashAnalysis) {
    match node.kind() {
        "command" => extract_command(node, source, analysis),
        "pipeline" => {
            check_pipe_to_shell(node, source, analysis);
            check_base64_to_shell(node, source, analysis);
        }
        "command_substitution" | "process_substitution" => {
            analysis.has_command_substitution = true;
        }
        "file_redirect" => extract_redirect(node, source, analysis),
        _ => {}
    }

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        walk(child, source, analysis);
    }
}

fn extract_command(node: Node, source: &[u8], analysis: &mut BashAnalysis) {
    let name_node = match node.child_by_field_name("name") {
        Some(n) => n,
        None => return,
    };
    let name = normalize_command_name(&node_text(name_node, source));

    let mut args = Vec::new();
    let mut path_args = Vec::new();

    let mut cursor = node.walk();
    for child in node.children_by_field_name("argument", &mut cursor) {
        let text = extract_arg_text(child, source);
        if is_path_arg(&text) {
            path_args.push(text.clone());
        }
        args.push(text);
    }

    analysis.commands.push(Command {
        name,
        args,
        path_args,
    });
}

fn check_pipe_to_shell(node: Node, source: &[u8], analysis: &mut BashAnalysis) {
    let mut commands_in_pipe: Vec<Node> = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "command" {
            commands_in_pipe.push(child);
        }
    }

    if commands_in_pipe.len() < 2 {
        return;
    }

    let last = commands_in_pipe.last().unwrap();
    let last_is_shell = last.child_by_field_name("name").is_some_and(|n| {
        let name = normalize_command_name(&node_text(n, source));
        SHELL_COMMANDS.contains(&name.as_str())
    });

    if !last_is_shell {
        return;
    }

    for cmd_node in &commands_in_pipe[..commands_in_pipe.len() - 1] {
        let name = cmd_node
            .child_by_field_name("name")
            .map(|n| normalize_command_name(&node_text(n, source)));
        if let Some(ref n) = name {
            if NETWORK_LOADERS.contains(&n.as_str()) {
                analysis.has_pipe_to_shell = true;
                return;
            }
        }
    }
}

fn check_base64_to_shell(node: Node, source: &[u8], analysis: &mut BashAnalysis) {
    let mut commands_in_pipe: Vec<Node> = Vec::new();
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "command" {
            commands_in_pipe.push(child);
        }
    }

    if commands_in_pipe.len() < 2 {
        return;
    }

    let last = commands_in_pipe.last().unwrap();
    let last_is_shell = last.child_by_field_name("name").is_some_and(|n| {
        let name = normalize_command_name(&node_text(n, source));
        SHELL_COMMANDS.contains(&name.as_str())
    });

    if !last_is_shell {
        return;
    }

    for cmd_node in &commands_in_pipe[..commands_in_pipe.len() - 1] {
        let name = cmd_node
            .child_by_field_name("name")
            .map(|n| normalize_command_name(&node_text(n, source)));
        if name.as_deref() != Some("base64") {
            continue;
        }
        let mut arg_cursor = cmd_node.walk();
        for arg_child in cmd_node.children_by_field_name("argument", &mut arg_cursor) {
            let text = node_text(arg_child, source);
            if text == "-d" || text == "--decode" {
                analysis.has_base64_to_shell = true;
                return;
            }
        }
    }
}

fn detect_fork_bomb(command: &str) -> bool {
    let normalized: String = command.chars().filter(|c| !c.is_whitespace()).collect();

    if normalized.contains("(){") && normalized.contains("|") && normalized.contains("&}") {
        let s = command.trim();

        if let Some(paren_pos) = s.find('(') {
            let fname = s[..paren_pos].trim();
            if !fname.is_empty() && fname.len() <= 10 && s.contains(&format!("{}|{}", fname, fname))
            {
                return true;
            }
        }
    }
    false
}

fn extract_redirect(node: Node, source: &[u8], analysis: &mut BashAnalysis) {
    if let Some(dest) = node.child_by_field_name("destination") {
        let target = extract_arg_text(dest, source);
        analysis.redirects.push(Redirect { target });
    }
}

fn node_text(node: Node, source: &[u8]) -> String {
    node.utf8_text(source).unwrap_or_default().to_string()
}

fn normalize_command_name(raw: &str) -> String {
    let unquoted = unquote(raw);
    match unquoted.rfind('/') {
        Some(pos) => unquoted[pos + 1..].to_string(),
        None => unquoted,
    }
}

fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

fn extract_arg_text(node: Node, source: &[u8]) -> String {
    unquote(&node_text(node, source))
}

fn is_path_arg(arg: &str) -> bool {
    arg.starts_with('/')
        || arg.starts_with("./")
        || arg.starts_with("../")
        || arg.starts_with("~/")
        || arg == "."
        || arg == ".."
        || arg == "~"
}
