# Starail CLI Implementation Details

This file captures concrete implementation decisions that are more specific than the product-level notes in `AGENTS.md`.

## Platform Scope

- The first version supports Linux only.
- Other operating systems are intentionally ignored until the Linux workflow is useful and stable.
- In headless or server-style Linux environments, "system proxy" means shell proxy environment variables managed for the current user.
- Starail does not manage desktop environment proxy settings, systemd service environments, cron environments, Docker daemon proxy settings, package-manager proxy settings, firewall rules, or DNS settings in the first version.

## Terminal UI

- Running `starail` without a subcommand opens the interactive terminal UI.
- `starail tui` is an explicit alias for the same UI.
- The TUI is the primary daily-use path.
- Explicit commands remain available for scripting and direct use.
- If `mihomo` is missing on first run, the TUI asks the user to confirm before downloading and installing it.

## Profiles And Subscriptions

- A profile is a local config entry that Starail can select and run.
- A local YAML config is a profile.
- A subscription URL creates a subscription-backed profile.
- Subscription-backed profiles store their source URL so they can be refreshed later.
- `profile use <name>` selects the current active profile.
- `profile list|add|remove` manages all profiles.
- `subscribe add <url>` creates a subscription-backed profile.
- `subscribe update [name]` refreshes subscription-backed profiles only.
- Avoid `profile update` in the first version to prevent overlap with `subscribe update`.

## Starail State

- Store Starail-managed state under `~/.starail`.
- Store the main Starail config at `~/.starail/config.yaml`.
- Store downloaded `mihomo` cores separately from user profiles, for example under `~/.starail/bin`.
- The Starail config should track runtime choices such as the active profile, mixed port, controller address, controller secret, latency-test URL, latency-test timeout, and shell-proxy enabled state.

## Runtime Mihomo Config

- Preserve original local configs and downloaded subscription configs.
- Generate a Starail-managed runtime config copy before starting `mihomo`.
- Use `mixed-port` for local HTTP/SOCKS proxy traffic.
- If the selected profile does not define `mixed-port`, inject a default `mixed-port: 7890` into the runtime config copy.
- Ensure the runtime config has the external controller settings needed for status, mode switching, node testing, and node selection.

## Shell Proxy

- `system-proxy on|off|status` manages a Starail-owned block in `~/.bashrc`.
- Only modify the Starail-owned block; do not rewrite unrelated user shell configuration.
- Proxy variables should point at `http://127.0.0.1:<mixed-port>`.
- Write both lowercase and uppercase proxy variables for compatibility:
  - `http_proxy`
  - `https_proxy`
  - `all_proxy`
  - `HTTP_PROXY`
  - `HTTPS_PROXY`
  - `ALL_PROXY`
- Also write `no_proxy` and `NO_PROXY` for local addresses.
- Changes to `~/.bashrc` affect new shells; already-running processes are not updated automatically.

## Node Testing

- `proxy test` tests all nodes in the current active subscription-backed profile.
- Use the mihomo external controller delay API when available.
- The default latency-test URL is `https://www.gstatic.com/generate_204`.
- The default timeout is 5000 ms.

## Minimal Product Boundary

- The first version should stay focused on the daily loop: install core, add/update subscription, start/stop mihomo, test nodes, switch nodes, switch mode, and toggle shell proxy.
- Do not build a complex Clash dashboard clone.
- Do not add advanced rule editing, traffic graphs, TUN/DNS management, or broad system integration until the minimal workflow is complete.
