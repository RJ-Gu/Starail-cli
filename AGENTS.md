# Starail CLI Agent Notes

## Project Summary

Starail CLI is a lightweight Linux terminal client for the local `mihomo` core. Its goal is to provide a small, reliable Clash-style workflow with both explicit CLI commands and a default interactive terminal UI, so users can manage subscriptions, test and switch nodes, and enable or disable shell proxy settings without memorizing `mihomo` commands or editing complex configs manually.

## Core Code Structure

```text
.
|-- Cargo.toml                  Rust crate metadata and dependencies.
|-- starail                     Thin clone-time launcher that runs cargo run.
|-- install.sh                  Builds the release binary and installs it to ~/.local/bin/starail.
|-- README.md                   User-facing setup and common command documentation.
|-- AGENTS.md                   Agent-facing product notes and codebase map.
|-- implementation-details.md   Concrete implementation decisions and constraints.
`-- src/
    |-- main.rs                 Program entrypoint and top-level command dispatch.
    |-- cli.rs                  clap command definitions and argument shapes.
    |-- config.rs               ~/.starail/config.yaml load/save/defaults.
    |-- paths.rs                Starail state paths under ~/.starail and profile path helpers.
    |-- platform.rs             Linux-only platform guard.
    |-- core.rs                 mihomo core discovery, download, install, and version handling.
    |-- runtime.rs              Runtime config generation, validation, start/stop/restart/logs.
    |-- controller.rs           mihomo external controller HTTP API client.
    |-- profile.rs              Local profile list/add/use/remove and profile metadata.
    |-- subscription.rs         Subscription fetch/add/update workflow.
    |-- proxy.rs                Proxy group/node listing, latency tests, and node selection.
    |-- status.rs               Status snapshot collection and printing.
    |-- system_proxy.rs         Starail-owned ~/.bashrc shell proxy block management.
    |-- tui.rs                  Default interactive terminal UI.
    `-- util.rs                 Shared validation, slug, timestamp, and JSON string helpers.
```

The repository should not contain a mihomo binary; Starail stores the managed core at `~/.starail/bin/mihomo`.

## Core Goals

- Start, stop, restart, and inspect the `mihomo` process.
- Bootstrap the local `mihomo` binary when it is missing.
- Run `mihomo` with a selected Clash/mihomo-compatible YAML config.
- Validate configuration files before starting the core.
- Show basic runtime status, logs, version, and controller connection info.
- Support simple profile management for local configs and subscription URLs.
- Fetch, update, and store subscription configs from user-provided links.
- Test proxy node latency and switch proxy groups or selected nodes through the mihomo external controller API when available.
- Enable and disable Linux shell proxy settings when explicitly requested by the user.
- Provide an interactive terminal UI as the default daily-use entry point, while keeping explicit commands available for scripting.
- Follow Clash-style profile semantics: local configs and subscriptions are both profiles, and subscriptions are profiles with update URLs.

## Command Shape

- `starail start -c <config>` starts mihomo with a config file.
- `starail` opens the interactive terminal UI by default.
- `starail stop` stops the managed mihomo process.
- `starail restart` restarts the current profile.
- `starail status` shows process, port, mode, and controller state.
- `starail logs` prints recent mihomo logs.
- `starail check -c <config>` validates a config.
- `starail core install|update|version` downloads and manages the local mihomo core.
- `starail profile list|use|add|remove` manages local and subscription-backed profiles.
- `starail subscribe add <url>` fetches a subscription and saves it as a profile.
- `starail subscribe update [name]` refreshes one or all subscription-backed profiles.
- `starail proxy list|test|select` lists, tests, and switches proxy groups or nodes.
- `starail mode rule|global|direct` changes proxy mode.
- `starail system-proxy on|off|status` manages Linux shell proxy settings.
- `starail tui` opens the interactive terminal UI for daily use.

## Implementation Principles

- Treat `mihomo` as the networking engine and keep the CLI focused on orchestration.
- Keep explicit commands stable and scriptable, but make the interactive terminal UI the easiest path for daily use.
- Target Linux first; other operating systems are out of scope for now.
- Store CLI state in a predictable user directory such as `~/.starail`.
- On first run, detect whether `mihomo` exists locally and ask for confirmation in the TUI before downloading it.
- Store downloaded cores separately from user profiles, for example under `~/.starail/bin`.
- When adding a subscription URL, fetch it, validate the resulting config, and keep the source URL for future updates.
- Never modify firewall, DNS, shell proxy, or privileged network settings implicitly; only change shell proxy settings through an explicit command or terminal UI action.
- Preserve compatibility with standard Clash and mihomo configuration formats.
- Keep errors actionable: explain the failed command, the config path, and the next useful step.

## Non-Goals

- No desktop GUI.
- No full Clash desktop-client replacement.
- No cross-platform support in the first version.
- No custom proxy protocol implementation.
- No complex rule editor.
- No hidden background behavior outside the managed mihomo process.

## Reference Direction

Existing Clash and mihomo GitHub clients can be used as product references for naming and workflows, but this project should stay smaller: a practical Linux terminal client for subscription management, node testing and switching, shell proxy toggles, and controlling a local mihomo core.
