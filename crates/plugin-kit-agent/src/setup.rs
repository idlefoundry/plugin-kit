//! Setup text for the bridge shipped beside an application.
use std::path::Path;
/// A POSIX shell argument, including paths with quotes, dollar signs or backticks.
pub fn claude_code(name: &str, bridge: &Path) -> String {
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\"'\"'"));
    format!(
        "claude mcp add --scope user {} -- {}",
        quote(name),
        quote(&bridge.to_string_lossy())
    )
}
pub fn json(name: &str, bridge: &Path) -> String {
    serde_json::json!({"mcpServers":{name:{"command":bridge,"args":[]}}}).to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn setup_preserves_quotes_spaces_and_shell_metacharacters() {
        let path = std::path::Path::new("/a b/c'$(touch nope)`file`");
        assert!(super::claude_code("test", path).contains("'\"'\"'"));
        let value: serde_json::Value =
            serde_json::from_str(&super::json("test", path)).expect("json");
        assert_eq!(
            value["mcpServers"]["test"]["command"],
            path.to_string_lossy().as_ref()
        );
    }
}
