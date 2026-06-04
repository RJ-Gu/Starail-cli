# Starail CLI

Starail CLI is a small Linux terminal client for managing a local `mihomo` core. It provides a default terminal UI plus scriptable commands for profiles, subscriptions, node testing, proxy selection, mode switching, logs, and Starail-owned shell proxy variables.

The repository intentionally does not contain a `mihomo` binary. Starail downloads and manages the core at `~/.starail/bin/mihomo`, stores profiles under `~/.starail/profiles`, and writes runtime state under `~/.starail`.

## Requirements

- Linux
- Rust/Cargo for running from a clone or installing from source

## Quick Start

Run directly from a clone:

```bash
chmod +x ./starail
./starail
```

Install a standalone binary so it keeps working after deleting the cloned repository:

```bash
chmod +x ./install.sh
./install.sh
starail
```

If `~/.local/bin` is not in your `PATH`, either add it or run:

```bash
~/.local/bin/starail
```

## Common Commands

```bash
starail core install
starail subscribe add <url> [name]
starail start
starail status
starail proxy list
starail proxy test
starail proxy select <group> <node>
starail mode rule
starail system-proxy on
starail stop
```

`starail` without arguments opens the interactive terminal UI. On first run, if no managed or PATH-provided `mihomo` core is found, the TUI asks before downloading it.

## State Layout

```text
~/.starail/config.yaml
~/.starail/bin/mihomo
~/.starail/profiles
~/.starail/runtime/config.yaml
~/.starail/logs/mihomo.log
```

Shell proxy management only edits a Starail-owned block in `~/.bashrc`. It affects new shells and does not change firewall, DNS, desktop proxy settings, systemd environments, Docker, or package manager proxy settings.
