use std::io::{self, Write};
use std::sync::mpsc;
use std::thread;
use std::time::Instant;

use anyhow::{bail, Result};
use serde_json::Value;

use crate::config::AppConfig;
use crate::controller;
use crate::paths::StarailPaths;
use crate::profile::{self, ProfileKind};

pub fn list(paths: &StarailPaths) -> Result<()> {
    paths.init()?;
    let json = controller::get_proxies(paths)
        .map_err(|_| controller::controller_required_error())
        .and_then(controller::ensure_controller_value)?;
    print_proxy_list(&json);
    Ok(())
}

pub fn test(paths: &StarailPaths) -> Result<()> {
    paths.init()?;
    let app_config = AppConfig::load(paths)?;
    let active = app_config
        .active_profile
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("no active profile selected"))?;
    let meta = profile::read_meta(paths, active)?;
    if meta.kind != ProfileKind::Subscription {
        bail!(
            "proxy test is scoped to the active subscription-backed profile. Active profile '{}' is type '{}'.",
            active,
            meta.kind.as_str()
        );
    }

    let json = controller::get_proxies(paths)
        .map_err(|_| controller::controller_required_error())
        .and_then(controller::ensure_controller_value)?;
    let nodes = node_names(&json);
    if nodes.is_empty() {
        bail!("no testable proxy nodes were found through the controller");
    }

    println!(
        "Testing {} nodes concurrently with {} timeout={}ms",
        nodes.len(),
        app_config.latency_test_url,
        app_config.latency_test_timeout
    );
    flush_stdout();
    test_nodes_concurrently(
        paths,
        nodes,
        &app_config.latency_test_url,
        app_config.latency_test_timeout,
    )
}

struct NodeTestResult {
    node: String,
    result: Result<String, String>,
    elapsed_ms: u128,
}

fn test_nodes_concurrently(
    paths: &StarailPaths,
    nodes: Vec<String>,
    test_url: &str,
    timeout: u64,
) -> Result<()> {
    let total = nodes.len();
    let (sender, receiver) = mpsc::channel();

    for node in nodes {
        let sender = sender.clone();
        let paths = paths.clone();
        let test_url = test_url.to_string();
        thread::spawn(move || {
            let started = Instant::now();
            let result = controller::delay(&paths, &node, &test_url, timeout)
                .map(delay_label)
                .map_err(|error| error.to_string());
            let _ = sender.send(NodeTestResult {
                node,
                result,
                elapsed_ms: started.elapsed().as_millis(),
            });
        });
    }
    drop(sender);

    let mut ok = false;
    let mut finished = 0usize;
    for result in receiver {
        finished += 1;
        match result.result {
            Ok(delay) => {
                println!(
                    "[{finished}/{total}] {:<48} {:>8}  finished={}ms",
                    result.node, delay, result.elapsed_ms
                );
                ok = true;
            }
            Err(error) => println!(
                "[{finished}/{total}] {:<48} {:>8}  finished={}ms  {}",
                result.node, "failed", result.elapsed_ms, error
            ),
        }
        flush_stdout();
    }

    if ok {
        Ok(())
    } else {
        bail!("all proxy delay tests failed")
    }
}

fn flush_stdout() {
    let _ = io::stdout().flush();
}

fn delay_label(value: Value) -> String {
    value
        .get("delay")
        .and_then(Value::as_u64)
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "ok".to_string())
}

pub fn select(paths: &StarailPaths, group: &str, node: &str) -> Result<()> {
    paths.init()?;
    if group.trim().is_empty() || node.trim().is_empty() {
        bail!("usage: starail proxy select <group> <node>");
    }

    controller::select_proxy(paths, group, node).map_err(|_| {
        anyhow::anyhow!(
            "failed to switch group '{}' to node '{}'. Check group/node names with 'starail proxy list'.",
            group,
            node
        )
    })?;
    println!("Selected '{node}' in group '{group}'.");
    Ok(())
}

fn print_proxy_list(json: &Value) {
    let Some(proxies) = json.get("proxies").and_then(Value::as_object) else {
        println!("{json}");
        return;
    };

    let mut names = proxies.keys().collect::<Vec<_>>();
    names.sort();

    for name in names {
        let Some(value) = proxies.get(name) else {
            continue;
        };
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let now = value.get("now").and_then(Value::as_str).unwrap_or_default();

        if let Some(children) = value.get("all").and_then(Value::as_array) {
            println!("[group] {name}  type={kind}  now={now}");
            for child in children.iter().filter_map(Value::as_str) {
                println!("  - {child}");
            }
        } else if !matches!(name.as_str(), "DIRECT" | "REJECT" | "GLOBAL") {
            let delay = last_delay(value)
                .map(|delay| format!("  last_delay={delay}ms"))
                .unwrap_or_default();
            println!("[node]  {name}  type={kind}{delay}");
        }
    }
}

fn node_names(json: &Value) -> Vec<String> {
    let Some(proxies) = json.get("proxies").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut names = proxies
        .iter()
        .filter_map(|(name, value)| {
            if matches!(name.as_str(), "DIRECT" | "REJECT" | "GLOBAL") {
                return None;
            }
            if value.get("all").and_then(Value::as_array).is_some() {
                return None;
            }
            Some(name.clone())
        })
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn last_delay(value: &Value) -> Option<u64> {
    value
        .get("history")?
        .as_array()?
        .last()?
        .get("delay")?
        .as_u64()
}
