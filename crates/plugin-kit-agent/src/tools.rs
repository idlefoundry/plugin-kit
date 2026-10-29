//! A product supplies one table for MCP schemas and the server's method allowlist.
use serde_json::{Value, json};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Runs {
    Bridge,
    Instance,
}
#[derive(Clone, Debug)]
pub struct Tool {
    pub name: &'static str,
    pub method: &'static str,
    pub description: &'static str,
    pub schema: Value,
    pub read_only: bool,
    pub runs: Runs,
}
impl Tool {
    pub fn json(&self) -> Value {
        json!({"name":self.name,"description":self.description,"inputSchema":self.schema,
            "annotations":{"readOnlyHint":self.read_only,"destructiveHint":false,"openWorldHint":false}})
    }
}
