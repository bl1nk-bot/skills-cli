use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use lazy_static::lazy_static;

// ============================================================================
// Constants and Regex Definitions
// ============================================================================

lazy_static! {
    static ref SUBAGENT_KEYWORD_RE: Regex = 
        Regex::new(r"\b(?:agent|subagent)s?\b").unwrap();
    static ref OPENCODE_SKILL_NAME_RE: Regex = 
        Regex::new(r"^[a-z0-9]+(-[a-z0-9]+)*$").unwrap();
}

const OPENCODE_SKILL_NAME_MAX: usize = 64;

const OPENCODE_PERMISSIONS: &[&str] = &[
    "read", "edit", "write", "bash", "grep", "glob", "list", "task",
    "skill", "lsp", "webfetch", "websearch", "external_directory",
    "todowrite", "question", "doom_loop",
];

// ============================================================================
// Structures
// ============================================================================

#[derive(Debug, Clone)]
pub struct PluginSource {
    pub name: String,
    pub skills: Vec<SkillSource>,
    pub agents: Vec<AgentSource>,
    pub commands: Vec<CommandSource>,
}

#[derive(Debug, Clone)]
pub struct SkillSource {
    pub name: String,
    pub dir: PathBuf,
    pub body: String,
    pub frontmatter: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct AgentSource {
    pub name: String,
    pub body: String,
    pub model: String,
    pub description: Option<String>,
    pub tools: Vec<String>,
    pub frontmatter: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct CommandSource {
    pub name: String,
    pub body: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EmitResult {
    pub written: Vec<PathBuf>,
    pub warnings: Vec<String>,
}

// ============================================================================
// Tool to Permission Mapping
// ============================================================================

fn get_tool_to_permission_map() -> HashMap<&'static str, &'static str> {
    vec![
        ("Read", "read"),
        ("Edit", "edit"),
        ("Write", "write"),
        ("Bash", "bash"),
        ("Grep", "grep"),
        ("Glob", "glob"),
        ("LS", "list"),
        ("Agent", "task"),
        ("Task", "task"),
        ("Skill", "skill"),
        ("LSP", "lsp"),
        ("WebFetch", "webfetch"),
        ("WebSearch", "websearch"),
        ("TodoWrite", "todowrite"),
        ("AskUserQuestion", "question"),
    ]
    .into_iter()
    .collect()
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Lowercase Claude tool names that appear as backticked identifiers
fn rewrite_body_lowercase_tools(body: &str) -> String {
    let mut out = body.to_string();
    
    // Example tool mappings - adjust based on TOOL_NAME_MAPS
    let tool_mappings = vec![
        ("Read", "read"),
        ("Write", "write"),
        ("Bash", "bash"),
        // Add more mappings as needed
    ];
    
    for (camel, replacement) in tool_mappings {
        let pattern = format!("`{}`", camel);
        let replacement_str = format!("`{}`", replacement);
        out = out.replace(&pattern, &replacement_str);
    }
    
    out
}

/// Convert source `tools:` allowlist to OpenCode permission block
fn build_permission_block(tools: &[String], has_tools_field: bool) -> HashMap<String, String> {
    let mut result = HashMap::new();
    let tool_map = get_tool_to_permission_map();
    let mut base_capabilities = HashSet::new();
    base_capabilities.insert("skill");
    base_capabilities.insert("task");

    if !has_tools_field {
        // Source author didn't set `tools:` — unrestricted
        return result;
    }

    if tools.is_empty() {
        // Explicit `tools: []` — locked down agent
        for perm in OPENCODE_PERMISSIONS {
            result.insert(
                perm.to_string(),
                if base_capabilities.contains(perm) {
                    "allow".to_string()
                } else {
                    "deny".to_string()
                },
            );
        }
        return result;
    }

    let mut granted = HashSet::new();
    for tool in tools {
        if let Some(&perm) = tool_map.get(tool.as_str()) {
            granted.insert(perm);
        }
    }

    if granted.is_empty() {
        // All tools are MCP / unmappable
        return result;
    }

    granted.extend(&base_capabilities);
    for perm in OPENCODE_PERMISSIONS {
        result.insert(
            perm.to_string(),
            if granted.contains(perm) {
                "allow".to_string()
            } else {
                "deny".to_string()
            },
        );
    }

    result
}

/// Format frontmatter as YAML-like string
fn opencode_frontmatter(fm: &HashMap<String, Value>) -> String {
    let mut lines = vec!["---".to_string()];

    for (k, v) in fm {
        match v {
            Value::Object(map) => {
                lines.push(format!("{}:", k));
                for (sk, sv) in map {
                    lines.push(format!("  {}: {}", sk, sv));
                }
            }
            Value::Array(arr) => {
                lines.push(format!("{}:", k));
                for item in arr {
                    lines.push(format!("  - {}", item));
                }
            }
            Value::Bool(b) => {
                lines.push(format!("{}: {}", k, if *b { "true" } else { "false" }));
            }
            Value::Null => {}
            _ => {
                let value = v.to_string().replace('\n', " ").trim().to_string();
                lines.push(format!("{}: {}", k, value));
            }
        }
    }

    lines.push("---".to_string());
    lines.join("\n")
}

/// Generate OpenCode-safe skill ID
fn opencode_skill_id(plugin_name: &str, skill_name: &str) -> Result<String, String> {
    let skill_id = format!("{}-{}", plugin_name, skill_name);

    if skill_id.len() > OPENCODE_SKILL_NAME_MAX {
        return Err(format!(
            "OpenCode skill id `{}` is {} chars; limit is {}",
            skill_id,
            skill_id.len(),
            OPENCODE_SKILL_NAME_MAX
        ));
    }

    if !OPENCODE_SKILL_NAME_RE.is_match(&skill_id) {
        return Err(format!(
            "OpenCode skill id `{}` must match pattern",
            skill_id
        ));
    }

    Ok(skill_id)
}

// ============================================================================
// Main Adapter
// ============================================================================

pub struct OpenCodeAdapter {
    output_root: Option<PathBuf>,
    seen_skill_ids: HashMap<String, String>,
}

impl OpenCodeAdapter {
    pub const HARNESS_ID: &'static str = "opencode";

    pub fn new(output_root: Option<PathBuf>) -> Self {
        OpenCodeAdapter {
            output_root,
            seen_skill_ids: HashMap::new(),
        }
    }

    pub fn emit_plugin(&mut self, plugin: &PluginSource) -> EmitResult {
        let mut result = EmitResult::default();

        for skill in &plugin.skills {
            self.emit_skill(plugin, skill, &mut result);
        }
        for agent in &plugin.agents {
            self.emit_agent(plugin, agent, &mut result);
        }
        for cmd in &plugin.commands {
            self.emit_command(plugin, cmd, &mut result);
        }

        result
    }

    pub fn emit_global(&self, _plugins: &[PluginSource]) -> EmitResult {
        let mut result = EmitResult::default();

        let config = json!({
            "$schema": "https://opencode.ai/config.json"
        });

        // Simulate write operation
        result.written.push(PathBuf::from("opencode.json"));

        result
    }

    // ── Internal Methods ──────────────────────────────────────────────────

    fn emit_skill(&mut self, plugin: &PluginSource, skill: &SkillSource, result: &mut EmitResult) {
        let skill_id = match opencode_skill_id(&plugin.name, &skill.name) {
            Ok(id) => id,
            Err(e) => {
                result.warnings.push(e);
                return;
            }
        };

        let source_id = format!("{}/{}", plugin.name, skill.name);

        if let Some(existing) = self.seen_skill_ids.get(&skill_id) {
            if existing != &source_id {
                result.warnings.push(format!(
                    "OpenCode skill id collision for `{}`: {} and {}",
                    skill_id, existing, source_id
                ));
            }
            return;
        }

        self.seen_skill_ids.insert(skill_id.clone(), source_id);

        let skill_dir = PathBuf::from(".opencode/skills").join(&skill_id);
        let mut fm = skill.frontmatter.clone();
        fm.insert("name".to_string(), Value::String(skill_id));

        let body = format!("{}\n", rewrite_body_lowercase_tools(&skill.body).trim_end());
        let content = format!("{}\n\n{}", opencode_frontmatter(&fm), body);

        result.written.push(skill_dir.join("SKILL.md"));
    }

    fn emit_agent(&self, plugin: &PluginSource, agent: &AgentSource, result: &mut EmitResult) {
        let agent_id = format!("{}__{}", plugin.name, agent.name);
        let rel = PathBuf::from(".opencode/agents").join(format!("{}.md", agent_id));

        let mut fm: HashMap<String, Value> = HashMap::new();
        fm.insert("name".to_string(), Value::String(agent_id.clone()));
        fm.insert(
            "description".to_string(),
            Value::String(
                agent
                    .description
                    .clone()
                    .unwrap_or_else(|| format!("{} (from {})", agent.name, plugin.name)),
            ),
        );
        fm.insert("mode".to_string(), Value::String("subagent".to_string()));
        fm.insert("model".to_string(), Value::String(agent.model.clone()));

        let has_tools_field = !agent.tools.is_empty();
        let permission = build_permission_block(&agent.tools, has_tools_field);
        if !permission.is_empty() {
            let perm_json: HashMap<String, Value> = permission
                .into_iter()
                .map(|(k, v)| (k, Value::String(v)))
                .collect();
            fm.insert("permission".to_string(), Value::Object(perm_json.into_iter().collect()));
        }

        let body = format!("{}\n", rewrite_body_lowercase_tools(&agent.body).trim_end());
        let content = format!("{}\n\n{}", opencode_frontmatter(&fm), body);

        result.written.push(rel);
    }

    fn emit_command(&self, plugin: &PluginSource, cmd: &CommandSource, result: &mut EmitResult) {
        let cmd_id = format!("{}__{}", plugin.name, cmd.name);
        let rel = PathBuf::from(".opencode/commands").join(format!("{}.md", cmd_id));

        let mut fm: HashMap<String, Value> = HashMap::new();
        fm.insert(
            "description".to_string(),
            Value::String(
                cmd.description
                    .clone()
                    .unwrap_or_else(|| format!("{} (from {})", cmd.name, plugin.name)),
            ),
        );

        if let Some(hint) = &cmd.argument_hint {
            fm.insert("argument-hint".to_string(), Value::String(hint.clone()));
        }

        if SUBAGENT_KEYWORD_RE.is_match(&cmd.body) {
            fm.insert("subtask".to_string(), Value::Bool(true));
        }

        let body = format!("{}\n", rewrite_body_lowercase_tools(&cmd.body).trim_end());
        let content = format!("{}\n\n{}", opencode_frontmatter(&fm), body);

        result.written.push(rel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_opencode_skill_id() {
        let result = opencode_skill_id("myplugin", "myskill");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "myplugin-myskill");
    }

    #[test]
    fn test_build_permission_block() {
        let tools = vec!["Read".to_string(), "Write".to_string()];
        let perms = build_permission_block(&tools, true);
        assert_eq!(perms.get("read").map(|s| s.as_str()), Some("allow"));
        assert_eq!(perms.get("bash").map(|s| s.as_str()), Some("deny"));
    }
}
