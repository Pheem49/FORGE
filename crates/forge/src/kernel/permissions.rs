use std::collections::HashMap;

use serde_json::json;

use crate::config::{PermissionMode, PermissionsConfig};
use crate::error::{Error, ForgeError, Result, codes};

#[derive(Debug, Clone)]
enum CompiledPattern {
    Any,
    Prefix { raw: String, prefix_dot: String },
    Exact(String),
}

impl CompiledPattern {
    fn parse(raw: &str) -> Result<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(Error::Config {
                path: None,
                message: "permission pattern must not be empty".into(),
            });
        }
        if trimmed == "*" {
            return Ok(Self::Any);
        }
        if let Some(stem) = trimmed.strip_suffix(".*") {
            if stem.is_empty() || stem.contains('*') {
                return Err(Error::Config {
                    path: None,
                    message: format!("invalid permission wildcard pattern '{trimmed}'"),
                });
            }
            return Ok(Self::Prefix {
                raw: trimmed.to_string(),
                prefix_dot: format!("{stem}."),
            });
        }
        if trimmed.contains('*') {
            return Err(Error::Config {
                path: None,
                message: format!(
                    "unsupported wildcard syntax '{trimmed}' (use '*', 'namespace.*', or an exact command name)"
                ),
            });
        }
        Ok(Self::Exact(trimmed.to_string()))
    }

    fn matches(&self, cmd: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Prefix { prefix_dot, .. } => cmd.starts_with(prefix_dot),
            Self::Exact(exact) => cmd == exact,
        }
    }

    fn raw_non_any(&self) -> Option<&str> {
        match self {
            Self::Any => None,
            Self::Prefix { raw, .. } => Some(raw),
            Self::Exact(s) => Some(s),
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledWindowRules {
    allow: Vec<CompiledPattern>,
    deny: Vec<CompiledPattern>,
}

/// Compiled capability policy evaluated at the single IPC choke point (`kernel/ipc.rs`).
#[derive(Debug, Clone)]
pub(crate) struct PermissionPolicy {
    mode: PermissionMode,
    allow: Vec<CompiledPattern>,
    deny: Vec<CompiledPattern>,
    windows: HashMap<String, CompiledWindowRules>,
}

impl PermissionPolicy {
    pub(crate) fn compile(cfg: &PermissionsConfig) -> Result<Self> {
        let allow = compile_list(&cfg.allow)?;
        let deny = compile_list(&cfg.deny)?;
        let mut windows = HashMap::with_capacity(cfg.windows.len());
        for (label, w_ovr) in &cfg.windows {
            windows.insert(
                label.clone(),
                CompiledWindowRules {
                    allow: compile_list(&w_ovr.allow)?,
                    deny: compile_list(&w_ovr.deny)?,
                },
            );
        }
        Ok(Self {
            mode: cfg.mode,
            allow,
            deny,
            windows,
        })
    }

    /// Validates that configured patterns match at least one registered command (failing fast on
    /// typos in `enforce` mode, or returning warnings in `permissive` mode), and emits a startup
    /// diagnostic if `enforce` mode has an empty allow-list while user commands are registered.
    pub(crate) fn validate(
        &self,
        all_registered: &[&str],
        user_registered: &[&str],
    ) -> Result<Vec<String>> {
        let mut warnings = Vec::new();

        let all_patterns = self.allow.iter().chain(self.deny.iter()).chain(
            self.windows
                .values()
                .flat_map(|w| w.allow.iter().chain(w.deny.iter())),
        );

        for pat in all_patterns {
            if let Some(raw) = pat.raw_non_any()
                && !all_registered.iter().any(|cmd| pat.matches(cmd))
            {
                let msg = format!("permission pattern '{raw}' matches no registered command");
                match self.mode {
                    PermissionMode::Enforce => {
                        return Err(Error::Config {
                            path: None,
                            message: msg,
                        });
                    }
                    PermissionMode::Permissive => {
                        warnings.push(msg);
                    }
                }
            }
        }

        if self.mode == PermissionMode::Enforce
            && self.allow.is_empty()
            && self.windows.is_empty()
            && !user_registered.is_empty()
        {
            let mut sorted = user_registered.to_vec();
            sorted.sort_unstable();
            warnings.push(format!(
                "[permissions] is in strict 'enforce' mode with an empty allow-list; commands [{}] will be denied (run `forge permissions init` or configure [permissions] in forge.toml)",
                sorted.join(", ")
            ));
        }

        Ok(warnings)
    }

    /// Checks whether `window_label` is permitted to invoke `command`.
    /// Deny rules always take precedence over allow rules.
    pub(crate) fn check(&self, window_label: &str, command: &str) -> Result<(), ForgeError> {
        if self.deny.iter().any(|p| p.matches(command)) {
            return Err(denied_error(
                window_label,
                command,
                "command is explicitly blocked by [permissions].deny",
            ));
        }
        if let Some(win_rules) = self.windows.get(window_label)
            && win_rules.deny.iter().any(|p| p.matches(command))
        {
            return Err(denied_error(
                window_label,
                command,
                &format!(
                    "command is explicitly blocked by [permissions.windows.{window_label}].deny"
                ),
            ));
        }

        if self.mode == PermissionMode::Permissive {
            return Ok(());
        }

        let active_allow = match self.windows.get(window_label) {
            Some(win_rules) => &win_rules.allow,
            None => &self.allow,
        };

        if active_allow.iter().any(|p| p.matches(command)) {
            Ok(())
        } else {
            let hint = if self.windows.contains_key(window_label) {
                format!(
                    "add \"{command}\" to [permissions.windows.{window_label}].allow in forge.toml"
                )
            } else {
                format!(
                    "add \"{command}\" to [permissions].allow in forge.toml (or run `forge permissions init`)"
                )
            };
            Err(denied_error(window_label, command, &hint))
        }
    }
}

fn compile_list(raw_list: &[String]) -> Result<Vec<CompiledPattern>> {
    raw_list.iter().map(|s| CompiledPattern::parse(s)).collect()
}

fn denied_error(window: &str, command: &str, hint: &str) -> ForgeError {
    ForgeError::forbidden(
        codes::PERMISSION_DENIED,
        format!("window '{window}' is not permitted to invoke command '{command}'"),
    )
    .with_details(json!({
        "command": command,
        "window": window,
        "hint": hint,
    }))
}
