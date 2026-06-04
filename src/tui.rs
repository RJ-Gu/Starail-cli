use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde_json::Value;

use crate::config::AppConfig;
use crate::core;
use crate::paths::StarailPaths;
use crate::platform;
use crate::{controller, profile, runtime, status, system_proxy};

#[derive(Debug, Clone, Copy)]
enum Action {
    StartStop,
    Profiles,
    ListProxies,
    SwitchMode,
    ToggleShellProxy,
    Logs,
    InstallCore,
}

impl Action {
    fn label(self) -> &'static str {
        match self {
            Self::StartStop => "Start/stop mihomo",
            Self::Profiles => "Profiles",
            Self::ListProxies => "Proxy groups and nodes",
            Self::SwitchMode => "Switch mode",
            Self::ToggleShellProxy => "Toggle shell proxy",
            Self::Logs => "Logs",
            Self::InstallCore => "Check/update core",
        }
    }

    fn detail(self) -> &'static str {
        match self {
            Self::StartStop => "Start when stopped; stop when running. Restart by pressing twice.",
            Self::Profiles => "Manage local configs and subscription-backed profiles.",
            Self::ListProxies => "Inspect groups and selectable nodes through the controller.",
            Self::SwitchMode => "Set mihomo to rule, global, or direct mode.",
            Self::ToggleShellProxy => "Enable or disable Starail's shell proxy block.",
            Self::Logs => "Read recent mihomo runtime logs.",
            Self::InstallCore => "Check latest mihomo and update when needed.",
        }
    }
}

struct App {
    actions: Vec<Action>,
    selected: usize,
    status: Option<status::StatusSnapshot>,
    message: String,
    screen: Screen,
}

impl App {
    fn new() -> Self {
        Self {
            actions: vec![
                Action::StartStop,
                Action::Profiles,
                Action::ListProxies,
                Action::SwitchMode,
                Action::ToggleShellProxy,
                Action::Logs,
                Action::InstallCore,
            ],
            selected: 0,
            status: None,
            message: "Ready.".to_string(),
            screen: Screen::Home,
        }
    }

    fn refresh(&mut self, paths: &StarailPaths) {
        self.status = status::collect(paths).ok();
    }

    fn selected_action(&self) -> Action {
        self.actions[self.selected]
    }

    fn next_action(&mut self) {
        self.selected = (self.selected + 1) % self.actions.len();
    }

    fn previous_action(&mut self) {
        self.selected = if self.selected == 0 {
            self.actions.len() - 1
        } else {
            self.selected - 1
        };
    }
}

enum Screen {
    Home,
    Profiles(ProfilePage),
    Form(InputForm),
    Mode(ModePage),
    Logs(TextPage),
    Proxies(ProxyPage),
    ProxyGroup(ProxyGroupPage),
    Output(OutputPage),
    Confirm(ConfirmPage),
}

struct ProfilePage {
    profiles: Vec<profile::ProfileSummary>,
    selected: usize,
    message: String,
}

impl ProfilePage {
    fn load(paths: &StarailPaths) -> Result<Self> {
        Ok(Self {
            profiles: profile::list(paths)?,
            selected: 0,
            message: "Profiles loaded.".to_string(),
        })
    }

    fn selected_profile(&self) -> Option<&profile::ProfileSummary> {
        self.profiles.get(self.selected)
    }

    fn next(&mut self) {
        if !self.profiles.is_empty() {
            self.selected = (self.selected + 1) % self.profiles.len();
        }
    }

    fn previous(&mut self) {
        if self.profiles.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.profiles.len() - 1
        } else {
            self.selected - 1
        };
    }
}

struct InputForm {
    title: String,
    fields: Vec<InputField>,
    focus: usize,
    submit: FormSubmit,
    message: String,
}

struct InputField {
    label: &'static str,
    placeholder: &'static str,
    value: String,
}

#[derive(Debug, Clone, Copy)]
enum FormSubmit {
    AddSubscription,
    AddLocalProfile,
}

impl InputForm {
    fn subscription() -> Self {
        Self {
            title: "Add subscription".to_string(),
            fields: vec![
                InputField {
                    label: "Subscription URL",
                    placeholder: "https://example.com/subscription",
                    value: String::new(),
                },
                InputField {
                    label: "Profile name",
                    placeholder: "blank for automatic",
                    value: String::new(),
                },
            ],
            focus: 0,
            submit: FormSubmit::AddSubscription,
            message: "Enter moves through fields. Submit from the last field.".to_string(),
        }
    }

    fn local_profile() -> Self {
        Self {
            title: "Add local profile".to_string(),
            fields: vec![
                InputField {
                    label: "Profile name",
                    placeholder: "work",
                    value: String::new(),
                },
                InputField {
                    label: "Config path",
                    placeholder: "/home/user/config.yaml",
                    value: String::new(),
                },
            ],
            focus: 0,
            submit: FormSubmit::AddLocalProfile,
            message: "Enter moves through fields. Submit from the last field.".to_string(),
        }
    }

    fn next(&mut self) {
        self.focus = (self.focus + 1) % self.fields.len();
    }

    fn previous(&mut self) {
        self.focus = if self.focus == 0 {
            self.fields.len() - 1
        } else {
            self.focus - 1
        };
    }

    fn push(&mut self, value: char) {
        if let Some(field) = self.fields.get_mut(self.focus) {
            field.value.push(value);
        }
    }

    fn pop(&mut self) {
        if let Some(field) = self.fields.get_mut(self.focus) {
            field.value.pop();
        }
    }

    fn submit(&mut self) -> Option<CommandRequest> {
        match self.submit {
            FormSubmit::AddSubscription => {
                let url = self.fields[0].value.trim().to_string();
                if url.is_empty() {
                    self.message = "Subscription URL is required.".to_string();
                    return None;
                }

                let name = self.fields[1].value.trim().to_string();
                let mut args = vec!["subscribe".to_string(), "add".to_string(), url];
                if !name.is_empty() {
                    args.push(name);
                }
                Some(CommandRequest::new("Add subscription", args))
            }
            FormSubmit::AddLocalProfile => {
                let name = self.fields[0].value.trim().to_string();
                let config = self.fields[1].value.trim().to_string();
                if name.is_empty() {
                    self.message = "Profile name is required.".to_string();
                    return None;
                }
                if config.is_empty() {
                    self.message = "Config path is required.".to_string();
                    return None;
                }

                Some(CommandRequest::new(
                    "Add local profile",
                    vec!["profile".to_string(), "add".to_string(), name, config],
                ))
            }
        }
    }
}

struct ModePage {
    modes: Vec<&'static str>,
    selected: usize,
    current: Option<String>,
    message: String,
}

impl ModePage {
    fn load(paths: &StarailPaths) -> Self {
        let modes = vec!["rule", "global", "direct"];
        let (current, message) = match controller::get_configs(paths) {
            Ok(configs) => {
                let mode = configs
                    .get("mode")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                (mode, "Controller mode loaded.".to_string())
            }
            Err(_) => (
                None,
                "Controller is unreachable; selecting a mode will retry.".to_string(),
            ),
        };
        let selected = current
            .as_deref()
            .and_then(|mode| modes.iter().position(|candidate| *candidate == mode))
            .unwrap_or(0);

        Self {
            modes,
            selected,
            current,
            message,
        }
    }

    fn selected_mode(&self) -> &'static str {
        self.modes[self.selected]
    }

    fn next(&mut self) {
        self.selected = (self.selected + 1) % self.modes.len();
    }

    fn previous(&mut self) {
        self.selected = if self.selected == 0 {
            self.modes.len() - 1
        } else {
            self.selected - 1
        };
    }
}

struct TextPage {
    title: String,
    lines: Vec<String>,
    scroll: usize,
    message: String,
}

impl TextPage {
    fn logs(paths: &StarailPaths, lines: usize) -> Self {
        match tail_file(&paths.log_file, lines) {
            Ok(text) if !text.trim().is_empty() => Self {
                title: format!("Logs: {}", paths.log_file.display()),
                lines: text.lines().map(ToOwned::to_owned).collect(),
                scroll: 0,
                message: "Logs loaded.".to_string(),
            },
            Ok(_) => Self {
                title: format!("Logs: {}", paths.log_file.display()),
                lines: vec!["Log file is empty.".to_string()],
                scroll: 0,
                message: "Logs loaded.".to_string(),
            },
            Err(error) => Self {
                title: "Logs".to_string(),
                lines: vec![format!("{error:#}")],
                scroll: 0,
                message: "Unable to read logs.".to_string(),
            },
        }
    }

    fn scroll_down(&mut self, amount: usize) {
        self.scroll = (self.scroll + amount).min(self.lines.len().saturating_sub(1));
    }

    fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }
}

#[derive(Clone)]
struct ProxyItem {
    name: String,
    kind: String,
    children: Vec<String>,
}

impl ProxyItem {
    fn label(&self) -> String {
        format!(
            "{:<30} {:<12} {} nodes",
            self.name,
            empty_as_dash(&self.kind),
            self.children.len()
        )
    }
}

struct ProxyPage {
    items: Vec<ProxyItem>,
    selected: usize,
    message: String,
}

impl ProxyPage {
    fn load(paths: &StarailPaths) -> Result<Self> {
        let config = AppConfig::load(paths)?;
        let active = config
            .active_profile
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no active profile selected"))?;
        let profile_path = paths.profile_config_path(active);
        let text = fs::read_to_string(&profile_path)
            .with_context(|| format!("failed to read active profile {}", profile_path.display()))?;
        let yaml = serde_yaml::from_str::<serde_yaml::Value>(&text).with_context(|| {
            format!("failed to parse active profile {}", profile_path.display())
        })?;
        Ok(Self {
            items: proxy_groups_from_yaml(&yaml),
            selected: 0,
            message: format!("Loaded proxy groups from profile '{active}'."),
        })
    }

    fn selected_item(&self) -> Option<&ProxyItem> {
        self.items.get(self.selected)
    }

    fn next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    fn previous(&mut self) {
        if self.items.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.items.len() - 1
        } else {
            self.selected - 1
        };
    }
}

#[derive(Clone)]
struct ProxyGroupPage {
    group: ProxyItem,
    selected: usize,
    back_items: Vec<ProxyItem>,
    back_selected: usize,
    node_results: Vec<NodeTestStatus>,
    message: String,
}

#[derive(Clone)]
enum NodeTestStatus {
    Untested,
    Testing,
    Ok(String),
    Failed,
}

impl NodeTestStatus {
    fn display(&self) -> String {
        match self {
            Self::Untested => "-".to_string(),
            Self::Testing => "testing...".to_string(),
            Self::Ok(delay) => delay.clone(),
            Self::Failed => "failed".to_string(),
        }
    }

    fn is_testing(&self) -> bool {
        matches!(self, Self::Testing)
    }
}

impl ProxyGroupPage {
    fn next(&mut self) {
        if !self.group.children.is_empty() {
            self.selected = (self.selected + 1) % self.group.children.len();
        }
    }

    fn previous(&mut self) {
        if self.group.children.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.group.children.len() - 1
        } else {
            self.selected - 1
        };
    }

    fn selected_node(&self) -> Option<&str> {
        self.group.children.get(self.selected).map(String::as_str)
    }

    fn node_label(&self, index: usize, node: &str) -> String {
        let status = self
            .node_results
            .get(index)
            .map(NodeTestStatus::display)
            .unwrap_or_else(|| "-".to_string());
        let node = truncate_for_column(node, 44);
        format!("{node:<44} {status:>12}")
    }

    fn selected_status(&self) -> String {
        self.node_results
            .get(self.selected)
            .map(NodeTestStatus::display)
            .unwrap_or_else(|| "-".to_string())
    }

    fn is_testing(&self) -> bool {
        self.node_results.iter().any(NodeTestStatus::is_testing)
    }
}

struct OutputPage {
    title: String,
    lines: Vec<String>,
    scroll: usize,
    success: Option<bool>,
    message: String,
}

impl OutputPage {
    fn running(title: &str, args: &[String], elapsed: Duration, preview: Vec<String>) -> Self {
        let mut lines = vec![
            format!("Command: starail {}", args.join(" ")),
            format!("Elapsed: {}s", elapsed.as_secs()),
            "Status: still running".to_string(),
            "Press Ctrl-C to cancel this command.".to_string(),
        ];
        lines.extend(running_hint(args));
        if !preview.is_empty() {
            lines.push(String::new());
            lines.push("Recent output:".to_string());
            lines.extend(preview);
        }

        Self {
            title: title.to_string(),
            lines,
            scroll: 0,
            success: None,
            message: "Command is running.".to_string(),
        }
    }

    fn error(title: &str, message: String) -> Self {
        Self {
            title: title.to_string(),
            lines: vec![message],
            scroll: 0,
            success: Some(false),
            message: "Command failed.".to_string(),
        }
    }

    fn from_command(
        title: String,
        args: Vec<String>,
        result: io::Result<std::process::Output>,
    ) -> Self {
        match result {
            Ok(output) => {
                let mut lines = command_output_lines(&args, &output);
                if lines.is_empty() {
                    lines.push("Command completed with no output.".to_string());
                }
                Self {
                    title,
                    lines,
                    scroll: 0,
                    success: Some(output.status.success()),
                    message: if output.status.success() {
                        "Command completed.".to_string()
                    } else {
                        "Command failed.".to_string()
                    },
                }
            }
            Err(error) => Self {
                title,
                lines: vec![format!("failed to run starail: {error}")],
                scroll: 0,
                success: Some(false),
                message: "Command failed.".to_string(),
            },
        }
    }

    fn scroll_down(&mut self, amount: usize) {
        self.scroll = (self.scroll + amount).min(self.lines.len().saturating_sub(1));
    }

    fn scroll_up(&mut self, amount: usize) {
        self.scroll = self.scroll.saturating_sub(amount);
    }
}

#[derive(Clone)]
struct CommandRequest {
    title: String,
    args: Vec<String>,
}

impl CommandRequest {
    fn new(title: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            title: title.into(),
            args,
        }
    }

    fn static_args(title: &str, args: &[&str]) -> Self {
        Self {
            title: title.to_string(),
            args: args.iter().map(|value| value.to_string()).collect(),
        }
    }
}

#[derive(Clone)]
struct ConfirmPage {
    title: String,
    question: String,
    yes_label: String,
    no_label: String,
    selected_yes: bool,
    request: CommandRequest,
    cancel_message: String,
}

impl ConfirmPage {
    fn new(
        title: impl Into<String>,
        question: impl Into<String>,
        yes_label: impl Into<String>,
        no_label: impl Into<String>,
        request: CommandRequest,
        cancel_message: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            question: question.into(),
            yes_label: yes_label.into(),
            no_label: no_label.into(),
            selected_yes: true,
            request,
            cancel_message: cancel_message.into(),
        }
    }

    fn toggle(&mut self) {
        self.selected_yes = !self.selected_yes;
    }
}

enum Effect {
    None,
    Quit,
    Run(CommandRequest),
    TestProxyGroup(ProxyGroupPage),
}

pub fn run(paths: &StarailPaths) -> Result<()> {
    platform::require_linux()?;
    paths.init()?;

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut guard = TerminalGuard::new();

    let mut app = App::new();
    app.refresh(paths);
    if core::find_core(paths).is_none() {
        app.screen = Screen::Confirm(ConfirmPage::new(
            "Install mihomo core",
            format!(
                "mihomo core is missing. Download and install it to {}?",
                paths.core_file.display()
            ),
            "Install",
            "Later",
            CommandRequest::static_args("Install mihomo core", &["core", "install"]),
            "Skipped core install. Use Check/update core when you are ready.",
        ));
    }

    loop {
        terminal.draw(|frame| draw(frame, &app))?;
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }

        match event::read()? {
            Event::Key(key) => {
                let effect = handle_key(paths, &mut app, key);
                match effect {
                    Effect::None => {}
                    Effect::Quit => break,
                    Effect::Run(request) => run_command(&mut terminal, &mut app, paths, request)?,
                    Effect::TestProxyGroup(page) => {
                        test_proxy_group_nodes(&mut terminal, &mut app, paths, page)?
                    }
                }
            }
            Event::Resize(_, _) => {}
            _ => {}
        }
    }

    terminal.show_cursor()?;
    guard.leave()?;
    Ok(())
}

fn handle_key(paths: &StarailPaths, app: &mut App, key: KeyEvent) -> Effect {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Effect::Quit;
    }

    let screen = std::mem::replace(&mut app.screen, Screen::Home);
    let (next_screen, effect) = match screen {
        Screen::Home => handle_home_key(paths, app, key),
        Screen::Profiles(page) => handle_profiles_key(paths, page, key),
        Screen::Form(form) => handle_form_key(form, key),
        Screen::Mode(page) => handle_mode_key(page, key),
        Screen::Logs(page) => handle_text_key(page, key, TextPage::logs(paths, 200)),
        Screen::Proxies(page) => handle_proxies_key(paths, page, key),
        Screen::ProxyGroup(page) => handle_proxy_group_key(page, key),
        Screen::Output(page) => handle_output_key(page, key),
        Screen::Confirm(page) => handle_confirm_key(app, page, key),
    };

    app.screen = next_screen;
    effect
}

fn handle_home_key(paths: &StarailPaths, app: &mut App, key: KeyEvent) -> (Screen, Effect) {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => (Screen::Home, Effect::Quit),
        KeyCode::Char('r') => {
            app.refresh(paths);
            app.message = "Refreshed.".to_string();
            (Screen::Home, Effect::None)
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.next_action();
            (Screen::Home, Effect::None)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.previous_action();
            (Screen::Home, Effect::None)
        }
        KeyCode::Enter => action_effect(paths, app.selected_action()),
        _ => (Screen::Home, Effect::None),
    }
}

fn action_effect(paths: &StarailPaths, action: Action) -> (Screen, Effect) {
    match action {
        Action::StartStop => {
            if runtime::is_running(paths) {
                (
                    Screen::Home,
                    Effect::Run(CommandRequest::static_args("Stop mihomo", &["stop"])),
                )
            } else {
                (
                    Screen::Home,
                    Effect::Run(CommandRequest::static_args("Start mihomo", &["start"])),
                )
            }
        }
        Action::Profiles => (profiles_screen(paths), Effect::None),
        Action::ListProxies => (proxies_screen(paths), Effect::None),
        Action::SwitchMode => (Screen::Mode(ModePage::load(paths)), Effect::None),
        Action::ToggleShellProxy => {
            if system_proxy::block_present(paths) {
                (
                    Screen::Confirm(ConfirmPage::new(
                        "Disable shell proxy",
                        "Remove the Starail-owned proxy block from ~/.bashrc?",
                        "Disable",
                        "Cancel",
                        CommandRequest::static_args(
                            "Disable shell proxy",
                            &["system-proxy", "off"],
                        ),
                        "No change.",
                    )),
                    Effect::None,
                )
            } else {
                (
                    Screen::Confirm(ConfirmPage::new(
                        "Enable shell proxy",
                        "Add Starail-owned proxy variables to ~/.bashrc for new shells?",
                        "Enable",
                        "Cancel",
                        CommandRequest::static_args("Enable shell proxy", &["system-proxy", "on"]),
                        "No change.",
                    )),
                    Effect::None,
                )
            }
        }
        Action::Logs => (Screen::Logs(TextPage::logs(paths, 200)), Effect::None),
        Action::InstallCore => (
            Screen::Confirm(ConfirmPage::new(
                "Check/update core",
                "Check the latest mihomo release and update the managed core if needed?",
                "Check",
                "Cancel",
                CommandRequest::static_args("Check/update core", &["core", "install"]),
                "No change.",
            )),
            Effect::None,
        ),
    }
}

fn handle_profiles_key(
    paths: &StarailPaths,
    mut page: ProfilePage,
    key: KeyEvent,
) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => (Screen::Home, Effect::None),
        KeyCode::Char('r') => (profiles_screen(paths), Effect::None),
        KeyCode::Down | KeyCode::Char('j') => {
            page.next();
            (Screen::Profiles(page), Effect::None)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            page.previous();
            (Screen::Profiles(page), Effect::None)
        }
        KeyCode::Char('a') => (Screen::Form(InputForm::local_profile()), Effect::None),
        KeyCode::Char('s') => (Screen::Form(InputForm::subscription()), Effect::None),
        KeyCode::Char('U') => (
            Screen::Profiles(page),
            Effect::Run(CommandRequest::static_args(
                "Update subscriptions",
                &["subscribe", "update"],
            )),
        ),
        KeyCode::Char('u') => {
            let Some(selected) = page.selected_profile() else {
                page.message = "No profile selected.".to_string();
                return (Screen::Profiles(page), Effect::None);
            };
            if selected.kind != profile::ProfileKind::Subscription {
                page.message =
                    "Selected profile is local; only subscription profiles can be updated."
                        .to_string();
                return (Screen::Profiles(page), Effect::None);
            }
            let selected_name = selected.name.clone();
            (
                Screen::Profiles(page),
                Effect::Run(CommandRequest::new(
                    "Update subscription",
                    vec!["subscribe".to_string(), "update".to_string(), selected_name],
                )),
            )
        }
        KeyCode::Enter => {
            let Some(selected) = page.selected_profile() else {
                page.message = "No profile selected.".to_string();
                return (Screen::Profiles(page), Effect::None);
            };
            let selected_name = selected.name.clone();
            (
                Screen::Profiles(page),
                Effect::Run(CommandRequest::new(
                    "Use profile",
                    vec!["profile".to_string(), "use".to_string(), selected_name],
                )),
            )
        }
        KeyCode::Char('d') => {
            let Some(selected) = page.selected_profile() else {
                page.message = "No profile selected.".to_string();
                return (Screen::Profiles(page), Effect::None);
            };
            let selected_name = selected.name.clone();
            (
                Screen::Confirm(ConfirmPage::new(
                    "Remove profile",
                    format!("Remove profile '{}'?", selected_name),
                    "Remove",
                    "Cancel",
                    CommandRequest::new(
                        "Remove profile",
                        vec!["profile".to_string(), "remove".to_string(), selected_name],
                    ),
                    "No change.",
                )),
                Effect::None,
            )
        }
        _ => (Screen::Profiles(page), Effect::None),
    }
}

fn handle_form_key(mut form: InputForm, key: KeyEvent) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc => (Screen::Home, Effect::None),
        KeyCode::Tab | KeyCode::Down => {
            form.next();
            (Screen::Form(form), Effect::None)
        }
        KeyCode::BackTab | KeyCode::Up => {
            form.previous();
            (Screen::Form(form), Effect::None)
        }
        KeyCode::Enter => {
            if form.focus + 1 < form.fields.len() {
                form.next();
                return (Screen::Form(form), Effect::None);
            }

            match form.submit() {
                Some(request) => (Screen::Form(form), Effect::Run(request)),
                None => (Screen::Form(form), Effect::None),
            }
        }
        KeyCode::Backspace => {
            form.pop();
            (Screen::Form(form), Effect::None)
        }
        KeyCode::Char(value)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            form.push(value);
            (Screen::Form(form), Effect::None)
        }
        _ => (Screen::Form(form), Effect::None),
    }
}

fn handle_mode_key(mut page: ModePage, key: KeyEvent) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => (Screen::Home, Effect::None),
        KeyCode::Down | KeyCode::Char('j') => {
            page.next();
            (Screen::Mode(page), Effect::None)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            page.previous();
            (Screen::Mode(page), Effect::None)
        }
        KeyCode::Enter => {
            let selected_mode = page.selected_mode().to_string();
            (
                Screen::Mode(page),
                Effect::Run(CommandRequest::new(
                    "Switch mode",
                    vec!["mode".to_string(), selected_mode],
                )),
            )
        }
        _ => (Screen::Mode(page), Effect::None),
    }
}

fn handle_text_key(mut page: TextPage, key: KeyEvent, refreshed: TextPage) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => (Screen::Home, Effect::None),
        KeyCode::Char('r') => (Screen::Logs(refreshed), Effect::None),
        KeyCode::Down | KeyCode::Char('j') => {
            page.scroll_down(1);
            (Screen::Logs(page), Effect::None)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            page.scroll_up(1);
            (Screen::Logs(page), Effect::None)
        }
        KeyCode::PageDown => {
            page.scroll_down(12);
            (Screen::Logs(page), Effect::None)
        }
        KeyCode::PageUp => {
            page.scroll_up(12);
            (Screen::Logs(page), Effect::None)
        }
        KeyCode::Home | KeyCode::Char('g') => {
            page.scroll = 0;
            (Screen::Logs(page), Effect::None)
        }
        KeyCode::End | KeyCode::Char('G') => {
            page.scroll = page.lines.len().saturating_sub(1);
            (Screen::Logs(page), Effect::None)
        }
        _ => (Screen::Logs(page), Effect::None),
    }
}

fn handle_proxies_key(
    paths: &StarailPaths,
    mut page: ProxyPage,
    key: KeyEvent,
) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => (Screen::Home, Effect::None),
        KeyCode::Char('r') => (proxies_screen(paths), Effect::None),
        KeyCode::Down | KeyCode::Char('j') => {
            page.next();
            (Screen::Proxies(page), Effect::None)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            page.previous();
            (Screen::Proxies(page), Effect::None)
        }
        KeyCode::Enter => {
            let Some(item) = page.selected_item().cloned() else {
                page.message = "No proxy group selected.".to_string();
                return (Screen::Proxies(page), Effect::None);
            };
            let node_results = vec![NodeTestStatus::Untested; item.children.len()];
            let back_selected = page.selected;
            let back_items = page.items;
            (
                Screen::ProxyGroup(ProxyGroupPage {
                    group: item,
                    selected: 0,
                    back_items,
                    back_selected,
                    node_results,
                    message: "Nodes loaded from YAML group order.".to_string(),
                }),
                Effect::None,
            )
        }
        _ => (Screen::Proxies(page), Effect::None),
    }
}

fn handle_proxy_group_key(mut page: ProxyGroupPage, key: KeyEvent) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => {
            let selected = page
                .back_selected
                .min(page.back_items.len().saturating_sub(1));
            (
                Screen::Proxies(ProxyPage {
                    items: page.back_items,
                    selected,
                    message: "Proxy data loaded.".to_string(),
                }),
                Effect::None,
            )
        }
        KeyCode::Down | KeyCode::Char('j') => {
            page.next();
            (Screen::ProxyGroup(page), Effect::None)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            page.previous();
            (Screen::ProxyGroup(page), Effect::None)
        }
        KeyCode::Char('t') => {
            if page.group.children.is_empty() {
                page.message = "This group has no nodes to test.".to_string();
                (Screen::ProxyGroup(page), Effect::None)
            } else {
                let next = page.clone();
                (Screen::ProxyGroup(page), Effect::TestProxyGroup(next))
            }
        }
        KeyCode::Enter => {
            let Some(node) = page.selected_node() else {
                page.message = "No node selected.".to_string();
                return (Screen::ProxyGroup(page), Effect::None);
            };
            let group_name = page.group.name.clone();
            let node_name = node.to_string();
            (
                Screen::ProxyGroup(page),
                Effect::Run(CommandRequest::new(
                    "Select proxy node",
                    vec![
                        "proxy".to_string(),
                        "select".to_string(),
                        group_name,
                        node_name,
                    ],
                )),
            )
        }
        _ => (Screen::ProxyGroup(page), Effect::None),
    }
}

fn handle_output_key(mut page: OutputPage, key: KeyEvent) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter | KeyCode::Backspace => {
            (Screen::Home, Effect::None)
        }
        KeyCode::Down | KeyCode::Char('j') => {
            page.scroll_down(1);
            (Screen::Output(page), Effect::None)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            page.scroll_up(1);
            (Screen::Output(page), Effect::None)
        }
        KeyCode::PageDown => {
            page.scroll_down(12);
            (Screen::Output(page), Effect::None)
        }
        KeyCode::PageUp => {
            page.scroll_up(12);
            (Screen::Output(page), Effect::None)
        }
        _ => (Screen::Output(page), Effect::None),
    }
}

fn handle_confirm_key(app: &mut App, mut page: ConfirmPage, key: KeyEvent) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('n') => {
            app.message = page.cancel_message.clone();
            (Screen::Home, Effect::None)
        }
        KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::Char('h') | KeyCode::Char('l') => {
            page.toggle();
            (Screen::Confirm(page), Effect::None)
        }
        KeyCode::Char('y') => (Screen::Confirm(page.clone()), Effect::Run(page.request)),
        KeyCode::Enter => {
            if page.selected_yes {
                (Screen::Confirm(page.clone()), Effect::Run(page.request))
            } else {
                app.message = page.cancel_message.clone();
                (Screen::Home, Effect::None)
            }
        }
        _ => (Screen::Confirm(page), Effect::None),
    }
}

fn run_command(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    paths: &StarailPaths,
    request: CommandRequest,
) -> Result<()> {
    app.screen = Screen::Output(OutputPage::running(
        &request.title,
        &request.args,
        Duration::ZERO,
        Vec::new(),
    ));
    terminal.draw(|frame| draw(frame, app))?;

    let result = run_child_command(terminal, app, paths, &request);
    app.refresh(paths);
    app.screen = Screen::Output(OutputPage::from_command(
        request.title,
        request.args,
        result,
    ));
    Ok(())
}

fn run_child_command(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    paths: &StarailPaths,
    request: &CommandRequest,
) -> io::Result<std::process::Output> {
    let exe = std::env::current_exe()?;
    let (stdout_path, stderr_path) = command_capture_paths(paths);
    let stdout_file = File::create(&stdout_path)?;
    let stderr_file = File::create(&stderr_path)?;
    let mut child = Command::new(exe)
        .args(&request.args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()?;
    let started = Instant::now();

    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = fs::read(&stdout_path).unwrap_or_default();
            let stderr = fs::read(&stderr_path).unwrap_or_default();
            cleanup_capture_files(&stdout_path, &stderr_path);
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }

        app.screen = Screen::Output(OutputPage::running(
            &request.title,
            &request.args,
            started.elapsed(),
            command_preview(&stdout_path, &stderr_path),
        ));
        let _ = terminal.draw(|frame| draw(frame, app));

        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(key)
                    if key.modifiers.contains(KeyModifiers::CONTROL)
                        && key.code == KeyCode::Char('c') =>
                {
                    let _ = child.kill();
                    let _ = child.wait();
                    cleanup_capture_files(&stdout_path, &stderr_path);
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "command canceled by user",
                    ));
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
}

struct ProxyGroupTestUpdate {
    index: usize,
    result: std::result::Result<String, String>,
}

fn test_proxy_group_nodes(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    paths: &StarailPaths,
    mut page: ProxyGroupPage,
) -> Result<()> {
    if page.group.children.is_empty() {
        page.message = "This group has no nodes to test.".to_string();
        app.screen = Screen::ProxyGroup(page);
        return Ok(());
    }

    let app_config = AppConfig::load(paths)?;
    let total = page.group.children.len();
    page.node_results = vec![NodeTestStatus::Testing; total];
    page.message = format!(
        "Testing {total} nodes concurrently with {} timeout={}ms. Press Esc or Ctrl-C to cancel.",
        app_config.latency_test_url, app_config.latency_test_timeout
    );
    app.screen = Screen::ProxyGroup(page.clone());
    terminal.draw(|frame| draw(frame, app))?;

    let (sender, receiver) = mpsc::channel();
    for (index, node) in page.group.children.iter().cloned().enumerate() {
        let sender = sender.clone();
        let paths = paths.clone();
        let test_url = app_config.latency_test_url.clone();
        let timeout = app_config.latency_test_timeout;
        thread::spawn(move || {
            let result = controller::delay(&paths, &node, &test_url, timeout)
                .map(delay_label)
                .map_err(|error| error.to_string());
            let _ = sender.send(ProxyGroupTestUpdate { index, result });
        });
    }
    drop(sender);

    let mut finished = 0usize;
    let mut ok = 0usize;
    while finished < total {
        match receiver.recv_timeout(Duration::from_millis(120)) {
            Ok(update) => {
                finished += 1;
                match update.result {
                    Ok(delay) => {
                        ok += 1;
                        if let Some(status) = page.node_results.get_mut(update.index) {
                            *status = NodeTestStatus::Ok(delay);
                        }
                    }
                    Err(_) => {
                        if let Some(status) = page.node_results.get_mut(update.index) {
                            *status = NodeTestStatus::Failed;
                        }
                    }
                }
                page.message = format!("Testing nodes... {finished}/{total} finished, {ok} ok.");
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        app.screen = Screen::ProxyGroup(page.clone());
        terminal.draw(|frame| draw(frame, app))?;

        if event::poll(Duration::from_millis(1))? {
            match event::read()? {
                Event::Key(key) if is_proxy_test_cancel_key(&key) => {
                    clear_testing_statuses(&mut page);
                    page.message =
                        "Test cancelled; in-flight node checks may finish in background."
                            .to_string();
                    app.screen = Screen::ProxyGroup(page);
                    return Ok(());
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    clear_testing_statuses(&mut page);
    page.message = if ok == 0 {
        format!("Finished testing {finished}/{total} nodes; all failed.")
    } else {
        format!("Finished testing {finished}/{total} nodes; {ok} ok.")
    };
    app.screen = Screen::ProxyGroup(page);
    Ok(())
}

fn delay_label(value: Value) -> String {
    value
        .get("delay")
        .and_then(Value::as_u64)
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| "ok".to_string())
}

fn is_proxy_test_cancel_key(key: &KeyEvent) -> bool {
    (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
        || matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace
        )
}

fn clear_testing_statuses(page: &mut ProxyGroupPage) {
    for status in &mut page.node_results {
        if status.is_testing() {
            *status = NodeTestStatus::Untested;
        }
    }
}

fn draw(frame: &mut Frame<'_>, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(4),
        ])
        .split(frame.size());

    draw_header(frame, chunks[0], app, &screen_title(&app.screen));
    match &app.screen {
        Screen::Home => draw_home(frame, chunks[1], app),
        Screen::Profiles(page) => draw_profiles(frame, chunks[1], page),
        Screen::Form(form) => draw_form(frame, chunks[1], form),
        Screen::Mode(page) => draw_mode(frame, chunks[1], page),
        Screen::Logs(page) => draw_text_page(frame, chunks[1], page),
        Screen::Proxies(page) => draw_proxies(frame, chunks[1], page),
        Screen::ProxyGroup(page) => draw_proxy_group(frame, chunks[1], page),
        Screen::Output(page) => draw_output(frame, chunks[1], page),
        Screen::Confirm(page) => draw_confirm(frame, chunks[1], page),
    }
    draw_footer(frame, chunks[2], app);
}

fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App, title: &str) {
    let mut spans = vec![
        Span::styled(
            format!("Starail CLI {}", env!("CARGO_PKG_VERSION")),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            title.to_string(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];
    spans.extend(header_status_spans(app.status.as_ref()));
    let line = Line::from(spans);
    frame.render_widget(
        Paragraph::new(line).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let content = footer_content(app);
    frame.render_widget(
        Paragraph::new(vec![
            footer_message_line(content.message),
            footer_hint_line(&content.hints),
        ])
        .block(
            Block::default()
                .title("Message / Keys")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        area,
    );
}

#[derive(Clone, Copy)]
struct FooterHint {
    key: &'static str,
    label: &'static str,
}

struct FooterContent {
    message: String,
    hints: Vec<FooterHint>,
}

fn footer_message_line(message: String) -> Line<'static> {
    Line::from(vec![
        Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
        Span::styled(message, Style::default().fg(Color::White)),
    ])
}

fn footer_hint_line(hints: &[FooterHint]) -> Line<'static> {
    let mut spans = Vec::new();
    for (index, hint) in hints.iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            format!(" {} ", hint.key),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(hint.label, Style::default().fg(Color::Gray)));
    }
    Line::from(spans)
}

#[derive(Clone, Copy)]
enum StatusTone {
    Normal,
    Warning,
    Problem,
    Neutral,
}

fn header_status_spans(snapshot: Option<&status::StatusSnapshot>) -> Vec<Span<'static>> {
    match snapshot {
        Some(status) => vec![
            Span::styled("process: ", status_label_style()),
            Span::styled(
                status.process.clone(),
                status_value_style(process_tone(&status.process)),
            ),
            Span::styled(" | active: ", status_label_style()),
            Span::styled(
                status.active_profile.clone(),
                status_value_style(profile_tone(&status.active_profile)),
            ),
        ],
        None => vec![Span::styled(
            "status unavailable",
            status_value_style(StatusTone::Problem),
        )],
    }
}

fn home_status_lines(status: &status::StatusSnapshot) -> Vec<Line<'static>> {
    vec![
        status_line("Home", status.home.clone(), StatusTone::Neutral),
        status_line("Core", status.core.clone(), core_tone(&status.core)),
        status_line(
            "Core version",
            status.core_version.clone(),
            version_tone(&status.core_version),
        ),
        status_line(
            "Active profile",
            status.active_profile.clone(),
            profile_tone(&status.active_profile),
        ),
        status_line(
            "Current group/node",
            status.selected_proxy.clone(),
            selected_proxy_tone(&status.selected_proxy),
        ),
        status_line(
            "Mixed port",
            status.mixed_port.to_string(),
            StatusTone::Normal,
        ),
        status_line("Controller", status.controller.clone(), StatusTone::Neutral),
        status_line(
            "Process",
            status.process.clone(),
            process_tone(&status.process),
        ),
        status_line(
            "Controller state",
            status.controller_state.clone(),
            controller_tone(&status.controller_state),
        ),
    ]
}

fn status_line(label: &str, value: String, tone: StatusTone) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), status_label_style()),
        Span::styled(value, status_value_style(tone)),
    ])
}

fn status_label_style() -> Style {
    Style::default().fg(Color::Gray)
}

fn status_value_style(tone: StatusTone) -> Style {
    let style = match tone {
        StatusTone::Normal => Style::default().fg(Color::LightGreen),
        StatusTone::Warning => Style::default().fg(Color::Yellow),
        StatusTone::Problem => Style::default().fg(Color::LightRed),
        StatusTone::Neutral => Style::default().fg(Color::White),
    };

    match tone {
        StatusTone::Warning | StatusTone::Problem => style.add_modifier(Modifier::BOLD),
        StatusTone::Normal | StatusTone::Neutral => style,
    }
}

fn core_tone(value: &str) -> StatusTone {
    if value == "missing" {
        StatusTone::Problem
    } else {
        StatusTone::Normal
    }
}

fn version_tone(value: &str) -> StatusTone {
    if matches!(value, "unavailable" | "unknown") {
        StatusTone::Warning
    } else {
        StatusTone::Normal
    }
}

fn profile_tone(value: &str) -> StatusTone {
    if value == "none" {
        StatusTone::Warning
    } else {
        StatusTone::Normal
    }
}

fn selected_proxy_tone(value: &str) -> StatusTone {
    match value {
        "unavailable" => StatusTone::Problem,
        "none" => StatusTone::Warning,
        _ => StatusTone::Normal,
    }
}

fn process_tone(value: &str) -> StatusTone {
    if value.starts_with("running") {
        StatusTone::Normal
    } else {
        StatusTone::Problem
    }
}

fn controller_tone(value: &str) -> StatusTone {
    if value.starts_with("reachable") {
        StatusTone::Normal
    } else {
        StatusTone::Problem
    }
}

fn draw_home(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(area);

    let status_lines = match &app.status {
        Some(status) => home_status_lines(status),
        None => vec![Line::from(Span::styled(
            "Status unavailable.",
            status_value_style(StatusTone::Problem),
        ))],
    };
    let status = Paragraph::new(status_lines)
        .wrap(Wrap { trim: false })
        .block(page_block("Status"));
    frame.render_widget(status, body[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(5)])
        .split(body[1]);

    let items = app
        .actions
        .iter()
        .map(|action| ListItem::new(format!("{:<24} {}", action.label(), action.detail())))
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    state.select(Some(app.selected));
    let actions = List::new(items)
        .block(page_block("Daily Actions"))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(actions, right[0], &mut state);

    let selected = app.selected_action();
    let detail = Paragraph::new(vec![
        Line::from(Span::styled(
            selected.label(),
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(selected.detail()),
    ])
    .wrap(Wrap { trim: false })
    .block(page_block("Selected"));
    frame.render_widget(detail, right[1]);
}

fn draw_profiles(frame: &mut Frame<'_>, area: Rect, page: &ProfilePage) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(area);

    let items = if page.profiles.is_empty() {
        vec![ListItem::new("No profiles yet.")]
    } else {
        page.profiles
            .iter()
            .map(|profile| {
                let active = if profile.active { "*" } else { " " };
                ListItem::new(format!(
                    "{} {:<30} {:<12}",
                    active,
                    profile.name,
                    profile.kind.as_str()
                ))
            })
            .collect::<Vec<_>>()
    };
    let mut state = ListState::default();
    if !page.profiles.is_empty() {
        state.select(Some(page.selected));
    }
    let list = List::new(items)
        .block(page_block("Profiles"))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, body[0], &mut state);

    let detail_lines = match page.selected_profile() {
        Some(profile) => vec![
            Line::from(vec![
                Span::styled("Name: ", Style::default().fg(Color::Gray)),
                Span::raw(profile.name.clone()),
            ]),
            Line::from(vec![
                Span::styled("Type: ", Style::default().fg(Color::Gray)),
                Span::raw(profile.kind.as_str()),
            ]),
            Line::from(vec![
                Span::styled("Active: ", Style::default().fg(Color::Gray)),
                Span::raw(if profile.active { "yes" } else { "no" }),
            ]),
            Line::from(vec![
                Span::styled("Source: ", Style::default().fg(Color::Gray)),
                Span::raw(profile.source_url.as_deref().unwrap_or("-").to_string()),
            ]),
        ],
        None => vec![
            Line::from("Add a local config profile or a subscription-backed profile."),
            Line::from("Profiles are stored under ~/.starail/profiles."),
        ],
    };
    let detail = Paragraph::new(detail_lines)
        .wrap(Wrap { trim: false })
        .block(page_block("Details"));
    frame.render_widget(detail, body[1]);
}

fn draw_form(frame: &mut Frame<'_>, area: Rect, form: &InputForm) {
    let height = (form.fields.len() as u16 * 3 + 5).min(area.height);
    let modal = centered_rect(area, 70, height);
    frame.render_widget(Clear, modal);
    let block = page_block(&form.title);
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    let constraints = form
        .fields
        .iter()
        .map(|_| Constraint::Length(3))
        .chain(std::iter::once(Constraint::Min(2)))
        .collect::<Vec<_>>();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    for (index, field) in form.fields.iter().enumerate() {
        let focused = index == form.focus;
        let value = if field.value.is_empty() {
            Line::from(Span::styled(
                field.placeholder,
                Style::default().fg(Color::DarkGray),
            ))
        } else {
            let mut spans = vec![Span::raw(field.value.clone())];
            if focused {
                spans.push(Span::styled(" ", Style::default().bg(Color::Cyan)));
            }
            Line::from(spans)
        };
        let border_style = if focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let input = Paragraph::new(value).block(
            Block::default()
                .title(field.label)
                .borders(Borders::ALL)
                .border_style(border_style),
        );
        frame.render_widget(input, rows[index]);
    }
}

fn draw_mode(frame: &mut Frame<'_>, area: Rect, page: &ModePage) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);
    let items = page
        .modes
        .iter()
        .map(|mode| {
            let active = if page.current.as_deref() == Some(*mode) {
                "*"
            } else {
                " "
            };
            ListItem::new(format!("{active} {mode}"))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    state.select(Some(page.selected));
    let list = List::new(items)
        .block(page_block("Mode"))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, body[0], &mut state);

    let detail = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Current: ", Style::default().fg(Color::Gray)),
            Span::raw(page.current.as_deref().unwrap_or("unknown").to_string()),
        ]),
        Line::from(vec![
            Span::styled("Selected: ", Style::default().fg(Color::Gray)),
            Span::raw(page.selected_mode()),
        ]),
        Line::from(page.message.clone()),
    ])
    .wrap(Wrap { trim: false })
    .block(page_block("Controller"));
    frame.render_widget(detail, body[1]);
}

fn draw_text_page(frame: &mut Frame<'_>, area: Rect, page: &TextPage) {
    let text = page.lines.join("\n");
    let paragraph = Paragraph::new(text)
        .scroll((scroll_u16(page.scroll), 0))
        .wrap(Wrap { trim: false })
        .block(page_block(&page.title));
    frame.render_widget(paragraph, area);
}

fn draw_proxies(frame: &mut Frame<'_>, area: Rect, page: &ProxyPage) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    let items = if page.items.is_empty() {
        vec![ListItem::new("No proxy groups found in active profile.")]
    } else {
        page.items
            .iter()
            .map(|item| ListItem::new(item.label()))
            .collect::<Vec<_>>()
    };
    let mut state = ListState::default();
    if !page.items.is_empty() {
        state.select(Some(page.selected));
    }
    let list = List::new(items)
        .block(page_block("Proxy Groups"))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, body[0], &mut state);

    let lines = match page.selected_item() {
        Some(item) => proxy_detail_lines(item),
        None => vec![Line::from(
            "Select an active profile whose YAML contains proxy-groups.",
        )],
    };
    let detail = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(page_block("Details"));
    frame.render_widget(detail, body[1]);
}

fn draw_proxy_group(frame: &mut Frame<'_>, area: Rect, page: &ProxyGroupPage) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    let items = page
        .group
        .children
        .iter()
        .enumerate()
        .map(|(index, node)| ListItem::new(page.node_label(index, node)))
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    if !page.group.children.is_empty() {
        state.select(Some(page.selected));
    }
    let group_title = format!("Group: {}", page.group.name);
    let list = List::new(items)
        .block(page_block(&group_title))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, body[0], &mut state);

    let detail = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Type: ", Style::default().fg(Color::Gray)),
            Span::raw(empty_as_dash(&page.group.kind)),
        ]),
        Line::from(vec![
            Span::styled("Selected: ", Style::default().fg(Color::Gray)),
            Span::raw(page.selected_node().unwrap_or("-").to_string()),
        ]),
        Line::from(vec![
            Span::styled("Result: ", Style::default().fg(Color::Gray)),
            Span::raw(page.selected_status()),
        ]),
        Line::from(page.message.clone()),
    ])
    .wrap(Wrap { trim: false })
    .block(page_block("Selection"));
    frame.render_widget(detail, body[1]);
}

fn draw_output(frame: &mut Frame<'_>, area: Rect, page: &OutputPage) {
    let border = match page.success {
        Some(true) => Color::LightGreen,
        Some(false) => Color::Red,
        None => Color::Yellow,
    };
    let text = page.lines.join("\n");
    let paragraph = Paragraph::new(text)
        .scroll((scroll_u16(page.scroll), 0))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(page.title.as_str())
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border)),
        );
    frame.render_widget(paragraph, area);
}

fn draw_confirm(frame: &mut Frame<'_>, area: Rect, page: &ConfirmPage) {
    let modal = centered_rect(area, 68, 9);
    frame.render_widget(Clear, modal);
    let block = page_block(&page.title);
    let inner = block.inner(modal);
    frame.render_widget(block, modal);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(inner);
    let question = Paragraph::new(page.question.as_str())
        .alignment(Alignment::Center)
        .wrap(Wrap { trim: false });
    frame.render_widget(question, rows[0]);

    let buttons = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(22),
            Constraint::Percentage(6),
            Constraint::Percentage(22),
            Constraint::Percentage(25),
        ])
        .split(rows[1]);
    draw_button(
        frame,
        buttons[1],
        &page.yes_label,
        page.selected_yes,
        Color::LightGreen,
    );
    draw_button(
        frame,
        buttons[3],
        &page.no_label,
        !page.selected_yes,
        Color::Gray,
    );
}

fn draw_button(frame: &mut Frame<'_>, area: Rect, label: &str, selected: bool, color: Color) {
    let style = if selected {
        Style::default()
            .fg(Color::Black)
            .bg(color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    };
    let button = Paragraph::new(label)
        .style(style)
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::ALL).border_style(style));
    frame.render_widget(button, area);
}

fn profiles_screen(paths: &StarailPaths) -> Screen {
    match ProfilePage::load(paths) {
        Ok(page) => Screen::Profiles(page),
        Err(error) => Screen::Output(OutputPage::error("Profiles", format!("{error:#}"))),
    }
}

fn proxies_screen(paths: &StarailPaths) -> Screen {
    match ProxyPage::load(paths) {
        Ok(page) => Screen::Proxies(page),
        Err(error) => Screen::Output(OutputPage::error(
            "Proxy groups and nodes",
            format!("{error:#}"),
        )),
    }
}

fn proxy_groups_from_yaml(yaml: &serde_yaml::Value) -> Vec<ProxyItem> {
    let Some(groups) = yaml
        .get("proxy-groups")
        .and_then(serde_yaml::Value::as_sequence)
    else {
        return Vec::new();
    };

    groups
        .iter()
        .filter_map(|group| {
            let name = yaml_string_field(group, "name")?;
            let kind = yaml_string_field(group, "type").unwrap_or_default();
            let children = group
                .get("proxies")
                .and_then(serde_yaml::Value::as_sequence)
                .map(|nodes| {
                    nodes
                        .iter()
                        .filter_map(serde_yaml::Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            Some(ProxyItem {
                name,
                kind,
                children,
            })
        })
        .collect()
}

fn yaml_string_field(value: &serde_yaml::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_yaml::Value::as_str)
        .map(ToOwned::to_owned)
}

fn proxy_detail_lines(item: &ProxyItem) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled("Group: ", Style::default().fg(Color::Gray)),
            Span::raw(item.name.clone()),
        ]),
        Line::from(vec![
            Span::styled("Type: ", Style::default().fg(Color::Gray)),
            Span::raw(empty_as_dash(&item.kind)),
        ]),
        Line::from(vec![
            Span::styled("Nodes: ", Style::default().fg(Color::Gray)),
            Span::raw(item.children.len().to_string()),
        ]),
        Line::from("Press Enter to view the group's YAML node list."),
    ]
}

#[cfg(test)]
mod tui_tests {
    use super::*;

    #[test]
    fn proxy_groups_follow_yaml_order() {
        let yaml = serde_yaml::from_str::<serde_yaml::Value>(
            r#"
proxy-groups:
  - name: Auto
    type: url-test
    proxies: [A, B]
  - name: Manual
    type: select
    proxies:
      - C
      - D
proxies:
  - name: A
  - name: B
"#,
        )
        .expect("yaml should parse");

        let groups = proxy_groups_from_yaml(&yaml);

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Auto");
        assert_eq!(groups[0].children, ["A", "B"]);
        assert_eq!(groups[1].name, "Manual");
        assert_eq!(groups[1].children, ["C", "D"]);
    }
}

fn running_hint(args: &[String]) -> Vec<String> {
    if args.len() >= 2 && args[0] == "subscribe" && args[1] == "add" {
        vec![
            "This step fetches the subscription, saves a temp config, then validates it with mihomo.".to_string(),
            "The HTTP subscription fetch has a 60s timeout; validation depends on the mihomo core.".to_string(),
        ]
    } else if args.len() >= 2 && args[0] == "core" && args[1] == "install" {
        vec!["This step checks the latest mihomo release, compares versions, then downloads if needed.".to_string()]
    } else {
        Vec::new()
    }
}

fn command_capture_paths(paths: &StarailPaths) -> (PathBuf, PathBuf) {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let prefix = format!("tui-command-{}-{stamp}", std::process::id());
    (
        paths.tmp_dir.join(format!("{prefix}.stdout")),
        paths.tmp_dir.join(format!("{prefix}.stderr")),
    )
}

fn command_preview(stdout_path: &Path, stderr_path: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    append_preview_lines("stdout", stdout_path, &mut lines);
    append_preview_lines("stderr", stderr_path, &mut lines);
    let keep = 10usize;
    if lines.len() > keep {
        lines = lines.split_off(lines.len() - keep);
    }
    lines
}

fn append_preview_lines(label: &str, path: &Path, lines: &mut Vec<String>) {
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    if text.trim().is_empty() {
        return;
    }

    lines.push(format!("{label}:"));
    lines.extend(
        text.lines()
            .rev()
            .take(5)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .map(ToOwned::to_owned),
    );
}

fn cleanup_capture_files(stdout_path: &Path, stderr_path: &Path) {
    let _ = fs::remove_file(stdout_path);
    let _ = fs::remove_file(stderr_path);
}

fn command_output_lines(args: &[String], output: &std::process::Output) -> Vec<String> {
    let mut lines = Vec::new();
    if !output.status.success() {
        lines.push(format!(
            "Command failed: starail {} ({})",
            args.join(" "),
            output.status
        ));
        lines.push(String::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.trim().is_empty() {
        lines.extend(stdout.lines().map(ToOwned::to_owned));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.trim().is_empty() {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("stderr:".to_string());
        lines.extend(stderr.lines().map(ToOwned::to_owned));
    }

    lines
}

fn tail_file(path: &Path, lines: usize) -> Result<String> {
    let mut text = String::new();
    File::open(path)?.read_to_string(&mut text)?;

    if lines == 0 {
        return Ok(String::new());
    }

    let mut tail = text.lines().rev().take(lines).collect::<Vec<_>>();
    tail.reverse();
    let mut output = tail.join("\n");
    if !output.is_empty() {
        output.push('\n');
    }
    Ok(output)
}

fn page_block(title: &str) -> Block<'_> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
}

fn screen_title(screen: &Screen) -> String {
    match screen {
        Screen::Home => "Dashboard".to_string(),
        Screen::Profiles(_) => "Profiles".to_string(),
        Screen::Form(form) => form.title.clone(),
        Screen::Mode(_) => "Mode".to_string(),
        Screen::Logs(_) => "Logs".to_string(),
        Screen::Proxies(_) => "Proxies".to_string(),
        Screen::ProxyGroup(page) => format!("Proxy group: {}", page.group.name),
        Screen::Output(page) => page.title.clone(),
        Screen::Confirm(page) => page.title.clone(),
    }
}

fn footer_content(app: &App) -> FooterContent {
    match &app.screen {
        Screen::Home => FooterContent {
            message: app.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: "move",
                },
                FooterHint {
                    key: "Enter",
                    label: "open",
                },
                FooterHint {
                    key: "r",
                    label: "refresh",
                },
                FooterHint {
                    key: "q",
                    label: "quit",
                },
            ],
        },
        Screen::Profiles(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: "move",
                },
                FooterHint {
                    key: "Enter",
                    label: "use",
                },
                FooterHint {
                    key: "a",
                    label: "local",
                },
                FooterHint {
                    key: "s",
                    label: "subscription",
                },
                FooterHint {
                    key: "u",
                    label: "update",
                },
                FooterHint {
                    key: "U",
                    label: "update all",
                },
                FooterHint {
                    key: "d",
                    label: "remove",
                },
                FooterHint {
                    key: "r",
                    label: "refresh",
                },
                FooterHint {
                    key: "Esc",
                    label: "back",
                },
            ],
        },
        Screen::Form(form) => FooterContent {
            message: form.message.clone(),
            hints: vec![
                FooterHint {
                    key: "Tab",
                    label: "field",
                },
                FooterHint {
                    key: "Enter",
                    label: "next/submit",
                },
                FooterHint {
                    key: "Esc",
                    label: "cancel",
                },
            ],
        },
        Screen::Mode(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: "move",
                },
                FooterHint {
                    key: "Enter",
                    label: "apply",
                },
                FooterHint {
                    key: "Esc",
                    label: "back",
                },
            ],
        },
        Screen::Logs(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: "scroll",
                },
                FooterHint {
                    key: "PgUp/PgDn",
                    label: "fast",
                },
                FooterHint {
                    key: "r",
                    label: "refresh",
                },
                FooterHint {
                    key: "Esc",
                    label: "back",
                },
            ],
        },
        Screen::Proxies(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: "move",
                },
                FooterHint {
                    key: "Enter",
                    label: "open group",
                },
                FooterHint {
                    key: "r",
                    label: "refresh",
                },
                FooterHint {
                    key: "Esc",
                    label: "back",
                },
            ],
        },
        Screen::ProxyGroup(page) if page.is_testing() => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "Esc",
                    label: "cancel",
                },
                FooterHint {
                    key: "Ctrl-C",
                    label: "cancel",
                },
            ],
        },
        Screen::ProxyGroup(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: "move",
                },
                FooterHint {
                    key: "Enter",
                    label: "select node",
                },
                FooterHint {
                    key: "t",
                    label: "test group",
                },
                FooterHint {
                    key: "Esc",
                    label: "back",
                },
            ],
        },
        Screen::Output(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: "scroll",
                },
                FooterHint {
                    key: "PgUp/PgDn",
                    label: "fast",
                },
                FooterHint {
                    key: "Enter",
                    label: "back",
                },
                FooterHint {
                    key: "Esc",
                    label: "back",
                },
            ],
        },
        Screen::Confirm(_) => FooterContent {
            message: "Choose an answer.".to_string(),
            hints: vec![
                FooterHint {
                    key: "Left/Right",
                    label: "choose",
                },
                FooterHint {
                    key: "Enter",
                    label: "confirm",
                },
                FooterHint {
                    key: "Esc",
                    label: "cancel",
                },
            ],
        },
    }
}

fn centered_rect(area: Rect, percent_x: u16, height: u16) -> Rect {
    let vertical_margin = area.height.saturating_sub(height) / 2;
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(vertical_margin),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);
    let horizontal_margin = 100u16.saturating_sub(percent_x) / 2;
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(horizontal_margin),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(horizontal_margin),
        ])
        .split(popup_layout[1])[1]
}

fn scroll_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

fn truncate_for_column(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }

    if width <= 3 {
        return value.chars().take(width).collect();
    }

    let mut output = value.chars().take(width - 3).collect::<String>();
    output.push_str("...");
    output
}

fn empty_as_dash(value: &str) -> String {
    if value.trim().is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn new() -> Self {
        Self { active: true }
    }

    fn leave(&mut self) -> Result<()> {
        if self.active {
            disable_raw_mode()?;
            execute!(io::stdout(), LeaveAlternateScreen)?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
        }
    }
}
