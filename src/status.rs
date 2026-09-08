use std::fs;

use anyhow::Result;
use serde_json::Value;

use crate::config::AppConfig;
use crate::controller;
use crate::core;
use crate::paths::StarailPaths;
use crate::platform;
use crate::runtime;

#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    pub home: String,
    pub core: String,
    pub core_version: String,
    pub active_profile: String,
    pub selected_proxy: String,
    pub current_group: String,
    pub current_node: String,
    pub mixed_port: u16,
    pub controller: String,
    pub process: String,
    pub controller_state: String,
}

pub fn collect(paths: &StarailPaths) -> Result<StatusSnapshot> {
    paths.init()?;
    let app_config = AppConfig::load(paths)?;
    let process = if runtime::is_running(paths) {
        format!(
            "running (pid {})",
            runtime::read_pid(paths).unwrap_or_default()
        )
    } else {
        "stopped".to_string()
    };

    let active_profile = app_config
        .active_profile
        .as_deref()
        .unwrap_or("none")
        .to_string();
    let preferred_groups = preferred_proxy_groups(paths, &active_profile);
    let (controller_state, selected_proxy, current_group, current_node) =
        controller_status(paths, &preferred_groups);

    Ok(StatusSnapshot {
        home: paths.home.display().to_string(),
        core: core::describe(paths),
        core_version: core::version_string(paths).unwrap_or_else(|_| "unavailable".to_string()),
        active_profile,
        selected_proxy,
        current_group,
        current_node,
        mixed_port: app_config.mixed_port,
        controller: app_config.controller_base(),
        process,
        controller_state,
    })
}

fn controller_status(
    paths: &StarailPaths,
    preferred_groups: &[String],
) -> (String, String, String, String) {
    match controller::get_configs(paths) {
        Ok(configs) => {
            let (summary, group, node) = selected_proxy(paths, preferred_groups);
            (
                controller_state_from_configs(&configs),
                summary,
                group,
                node,
            )
        }
        Err(_) => (
            "unreachable".to_string(),
            "unavailable".to_string(),
            "unavailable".to_string(),
            "unavailable".to_string(),
        ),
    }
}

fn controller_state_from_configs(configs: &Value) -> String {
    let mode = configs
        .get("mode")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if mode.is_empty() {
        "reachable".to_string()
    } else {
        format!("reachable (mode: {mode})")
    }
}

fn selected_proxy(paths: &StarailPaths, preferred_groups: &[String]) -> (String, String, String) {
    match controller::get_proxies(paths) {
        Ok(proxies) => selected_proxy_details_from_json(&proxies, preferred_groups)
            .unwrap_or_else(|| ("none".to_string(), "none".to_string(), "none".to_string())),
        Err(_) => (
            "unavailable".to_string(),
            "unavailable".to_string(),
            "unavailable".to_string(),
        ),
    }
}

#[derive(Debug)]
struct ProxySelection {
    name: String,
    now: String,
}

impl ProxySelection {
    fn label(&self) -> String {
        format!("{} -> {}", self.name, self.now)
    }
}

#[cfg(test)]
fn selected_proxy_from_json(json: &Value, preferred_groups: &[String]) -> Option<String> {
    selected_proxy_details_from_json(json, preferred_groups).map(|(summary, _, _)| summary)
}

fn selected_proxy_details_from_json(
    json: &Value,
    preferred_groups: &[String],
) -> Option<(String, String, String)> {
    let selections = proxy_selections_from_json(json)?;
    let mut visible = preferred_proxy_selections(&selections, preferred_groups);
    let using_preferred_order = !visible.is_empty();

    if visible.is_empty() {
        visible = selections
            .iter()
            .filter(|selection| !is_builtin_group(&selection.name))
            .collect::<Vec<_>>();
    }
    if visible.is_empty() {
        visible = selections.iter().collect::<Vec<_>>();
    }

    if !using_preferred_order {
        visible.sort_by(|left, right| {
            selection_rank(left)
                .cmp(&selection_rank(right))
                .then_with(|| left.name.cmp(&right.name))
        });
    }

    let total = visible.len();
    let first = visible.into_iter().next()?;
    let summary = if total > 1 {
        format!("{} (+{} groups)", first.label(), total - 1)
    } else {
        first.label()
    };
    Some((summary, first.name.clone(), first.now.clone()))
}

fn proxy_selections_from_json(json: &Value) -> Option<Vec<ProxySelection>> {
    let proxies = json.get("proxies")?.as_object()?;
    Some(
        proxies
            .iter()
            .filter_map(|(name, value)| {
                value.get("all").and_then(Value::as_array)?;
                let now = value.get("now").and_then(Value::as_str)?.trim();
                if now.is_empty() {
                    return None;
                }
                Some(ProxySelection {
                    name: name.clone(),
                    now: now.to_string(),
                })
            })
            .collect::<Vec<_>>(),
    )
}

fn preferred_proxy_selections<'a>(
    selections: &'a [ProxySelection],
    preferred_groups: &[String],
) -> Vec<&'a ProxySelection> {
    preferred_groups
        .iter()
        .filter_map(|group| selections.iter().find(|selection| selection.name == *group))
        .collect()
}

fn preferred_proxy_groups(paths: &StarailPaths, active_profile: &str) -> Vec<String> {
    if active_profile == "none" {
        return Vec::new();
    }

    let profile_path = paths.profile_config_path(active_profile);
    let Ok(text) = fs::read_to_string(profile_path) else {
        return Vec::new();
    };
    let Ok(yaml) = serde_yaml::from_str::<serde_yaml::Value>(&text) else {
        return Vec::new();
    };
    let Some(groups) = yaml
        .get("proxy-groups")
        .and_then(serde_yaml::Value::as_sequence)
    else {
        return Vec::new();
    };

    groups
        .iter()
        .filter_map(|group| {
            group
                .get("name")
                .and_then(serde_yaml::Value::as_str)
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn selection_rank(selection: &ProxySelection) -> usize {
    if is_builtin_group(&selection.name) {
        return 3;
    }

    let group = selection.name.to_ascii_lowercase();
    if matches!(group.as_str(), "proxy" | "proxies" | "select") {
        0
    } else if group.contains("proxy") || group.contains("select") {
        1
    } else if group.contains("auto") {
        2
    } else if is_builtin_node(&selection.now) {
        4
    } else {
        3
    }
}

fn is_builtin_group(name: &str) -> bool {
    matches!(name, "GLOBAL" | "DIRECT" | "REJECT")
}

fn is_builtin_node(name: &str) -> bool {
    matches!(name, "GLOBAL" | "DIRECT" | "REJECT")
}

pub fn print(paths: &StarailPaths) -> Result<()> {
    platform::require_linux()?;
    let status = collect(paths)?;

    println!("Starail home: {}", status.home);
    println!("Core: {}", status.core);
    println!("Core version: {}", status.core_version);
    println!("Active profile: {}", status.active_profile);
    println!("Current group/node: {}", status.selected_proxy);
    println!("Mixed port: {}", status.mixed_port);
    println!("Controller: {}", status.controller);
    println!("Process: {}", status.process);
    println!("Controller state: {}", status.controller_state);
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn selected_proxy_reports_group_and_node() {
        let json = json!({
            "proxies": {
                "Auto": { "all": ["A", "B"], "now": "B" },
                "A": { "type": "VLESS" },
                "B": { "type": "Hysteria2" }
            }
        });

        assert_eq!(
            selected_proxy_from_json(&json, &[]).as_deref(),
            Some("Auto -> B")
        );
    }

    #[test]
    fn selected_proxy_ignores_global_when_real_group_exists() {
        let json = json!({
            "proxies": {
                "Other": { "all": ["B"], "now": "B" },
                "GLOBAL": { "all": ["DIRECT"], "now": "DIRECT" }
            }
        });

        assert_eq!(
            selected_proxy_from_json(&json, &[]).as_deref(),
            Some("Other -> B")
        );
    }

    #[test]
    fn selected_proxy_follows_active_profile_group_order() {
        let json = json!({
            "proxies": {
                "Auto": { "all": ["A", "B"], "now": "B" },
                "Manual": { "all": ["C", "D"], "now": "D" },
                "GLOBAL": { "all": ["DIRECT"], "now": "DIRECT" }
            }
        });
        let preferred = vec!["Manual".to_string(), "Auto".to_string()];

        assert_eq!(
            selected_proxy_from_json(&json, &preferred).as_deref(),
            Some("Manual -> D (+1 groups)")
        );
    }

    #[test]
    fn selected_proxy_summarizes_extra_real_groups() {
        let json = json!({
            "proxies": {
                "Proxy": { "all": ["A"], "now": "A" },
                "Auto": { "all": ["B"], "now": "B" },
                "GLOBAL": { "all": ["DIRECT"], "now": "DIRECT" }
            }
        });

        assert_eq!(
            selected_proxy_from_json(&json, &[]).as_deref(),
            Some("Proxy -> A (+1 groups)")
        );
    }
}
