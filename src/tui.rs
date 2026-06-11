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
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::config::{app_language, AppConfig};
use crate::core;
use crate::i18n::{self, Language, Message};
use crate::paths::StarailPaths;
use crate::platform;
use crate::{controller, profile, runtime, status, system_proxy};

#[derive(Debug, Clone, Copy)]
enum Action {
    StartStop,
    Profiles,
    ListProxies,
    ToggleShellProxy,
    Settings,
}

impl Action {
    fn label(self, language: Language) -> &'static str {
        match self {
            Self::StartStop => language.tr(Message::StartStopMihomo),
            Self::Profiles => language.tr(Message::Profiles),
            Self::ListProxies => language.tr(Message::ProxyGroupsAndNodes),
            Self::ToggleShellProxy => language.tr(Message::ToggleShellProxy),
            Self::Settings => language.tr(Message::Settings),
        }
    }

    fn detail(self, language: Language) -> &'static str {
        match self {
            Self::StartStop => language.tr(Message::StartStopDetail),
            Self::Profiles => language.tr(Message::ProfilesDetail),
            Self::ListProxies => language.tr(Message::ProxyGroupsDetail),
            Self::ToggleShellProxy => language.tr(Message::ToggleShellProxyDetail),
            Self::Settings => language.tr(Message::SettingsDetail),
        }
    }
}

struct App {
    actions: Vec<Action>,
    selected: usize,
    status: Option<status::StatusSnapshot>,
    message: String,
    language: Language,
    screen: Screen,
}

impl App {
    fn new() -> Self {
        let language = i18n::detect();
        Self {
            actions: vec![
                Action::StartStop,
                Action::Profiles,
                Action::ListProxies,
                Action::ToggleShellProxy,
                Action::Settings,
            ],
            selected: 0,
            status: None,
            message: language.tr(Message::Ready).to_string(),
            language,
            screen: Screen::Home,
        }
    }

    fn refresh(&mut self, paths: &StarailPaths) {
        self.language = app_language(paths);
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
    Settings(SettingsPage),
    Language(LanguagePage),
    Profiles(ProfilePage),
    Form(InputForm),
    Mode(ModePage),
    Logs(TextPage),
    Proxies(ProxyPage),
    ProxyGroup(ProxyGroupPage),
    Output(OutputPage),
    Confirm(ConfirmPage),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsItemKind {
    Language,
    MixedPort,
    SwitchMode,
    Logs,
    CheckUpdateCore,
}

impl SettingsItemKind {
    fn label(self, language: Language) -> &'static str {
        match self {
            Self::Language => language.tr(Message::Language),
            Self::MixedPort => language.tr(Message::CustomPort),
            Self::SwitchMode => language.tr(Message::SwitchMode),
            Self::Logs => language.tr(Message::Logs),
            Self::CheckUpdateCore => language.tr(Message::CheckUpdateCore),
        }
    }

    fn summary(self, language: Language) -> &'static str {
        match self {
            Self::Language => language.tr(Message::LanguageSummary),
            Self::MixedPort => language.tr(Message::CustomPortSummary),
            Self::SwitchMode => language.tr(Message::SwitchModeSummary),
            Self::Logs => language.tr(Message::LogsSummary),
            Self::CheckUpdateCore => language.tr(Message::CheckUpdateCoreSummary),
        }
    }

    fn status(self, language: Language) -> &'static str {
        match self {
            Self::Language | Self::MixedPort => language.tr(Message::EditStatus),
            Self::SwitchMode | Self::Logs => language.tr(Message::OpenStatus),
            Self::CheckUpdateCore => language.tr(Message::ConfirmStatus),
        }
    }

    fn scope(self, language: Language) -> &'static str {
        match self {
            Self::Language => language.tr(Message::TerminalUi),
            Self::MixedPort => language.tr(Message::RuntimeConfig),
            Self::SwitchMode => language.tr(Message::MihomoController),
            Self::Logs => language.tr(Message::Runtime),
            Self::CheckUpdateCore => language.tr(Message::ManagedCore),
        }
    }

    fn detail(self, language: Language) -> &'static str {
        match self {
            Self::Language => language.tr(Message::LanguageDetail),
            Self::MixedPort => language.tr(Message::CustomPortDetail),
            Self::SwitchMode => language.tr(Message::SwitchModeDetail),
            Self::Logs => language.tr(Message::LogsDetail),
            Self::CheckUpdateCore => language.tr(Message::CheckUpdateCoreDetail),
        }
    }

    fn detail_lines(self, language: Language) -> Vec<Line<'static>> {
        vec![
            setting_detail_line(language.tr(Message::Name), self.label(language).to_string()),
            setting_detail_line(
                language.tr(Message::Status),
                self.status(language).to_string(),
            ),
            setting_detail_line(
                language.tr(Message::Scope),
                self.scope(language).to_string(),
            ),
            Line::from(self.detail(language)),
        ]
    }
}

struct SettingsPage {
    items: Vec<SettingsItemKind>,
    selected: usize,
    message: String,
}

impl SettingsPage {
    fn load(language: Language) -> Self {
        Self {
            items: vec![
                SettingsItemKind::Language,
                SettingsItemKind::MixedPort,
                SettingsItemKind::SwitchMode,
                SettingsItemKind::Logs,
                SettingsItemKind::CheckUpdateCore,
            ],
            selected: 0,
            message: language.tr(Message::SettingsLoaded).to_string(),
        }
    }

    fn selected_item(&self) -> Option<SettingsItemKind> {
        self.items.get(self.selected).copied()
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

struct LanguagePage {
    options: Vec<Language>,
    selected: usize,
    current: Language,
    message: String,
}

impl LanguagePage {
    fn load(current: Language) -> Self {
        let options = Language::ALL.to_vec();
        let selected = options
            .iter()
            .position(|language| *language == current)
            .unwrap_or(0);
        Self {
            options,
            selected,
            current,
            message: current.tr(Message::ChooseLanguage).to_string(),
        }
    }

    fn selected_language(&self) -> Language {
        self.options
            .get(self.selected)
            .copied()
            .unwrap_or(Language::English)
    }

    fn next(&mut self) {
        if !self.options.is_empty() {
            self.selected = (self.selected + 1) % self.options.len();
        }
    }

    fn previous(&mut self) {
        if self.options.is_empty() {
            return;
        }
        self.selected = if self.selected == 0 {
            self.options.len() - 1
        } else {
            self.selected - 1
        };
    }
}

struct ProfilePage {
    profiles: Vec<profile::ProfileSummary>,
    selected: usize,
    message: String,
}

impl ProfilePage {
    fn load(paths: &StarailPaths, language: Language) -> Result<Self> {
        Ok(Self {
            profiles: profile::list(paths)?,
            selected: 0,
            message: language.tr(Message::ProfilesLoaded).to_string(),
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
    language: Language,
}

struct InputField {
    label: &'static str,
    placeholder: &'static str,
    value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FormSubmit {
    AddSubscription,
    AddLocalProfile,
    SetMixedPort,
}

impl InputForm {
    fn subscription(language: Language) -> Self {
        Self {
            title: language.tr(Message::AddSubscription).to_string(),
            fields: vec![
                InputField {
                    label: language.tr(Message::SubscriptionUrl),
                    placeholder: "https://example.com/subscription",
                    value: String::new(),
                },
                InputField {
                    label: language.tr(Message::ProfileName),
                    placeholder: language.tr(Message::BlankForAutomatic),
                    value: String::new(),
                },
            ],
            focus: 0,
            submit: FormSubmit::AddSubscription,
            message: language.tr(Message::FormSubmitHint).to_string(),
            language,
        }
    }

    fn local_profile(language: Language) -> Self {
        Self {
            title: language.tr(Message::AddLocalProfile).to_string(),
            fields: vec![
                InputField {
                    label: language.tr(Message::ProfileName),
                    placeholder: "work",
                    value: String::new(),
                },
                InputField {
                    label: language.tr(Message::ConfigPath),
                    placeholder: "/home/user/config.yaml",
                    value: String::new(),
                },
            ],
            focus: 0,
            submit: FormSubmit::AddLocalProfile,
            message: language.tr(Message::FormSubmitHint).to_string(),
            language,
        }
    }

    fn mixed_port(paths: &StarailPaths, language: Language) -> Self {
        let port = AppConfig::load(paths)
            .map(|config| config.mixed_port)
            .unwrap_or_else(|_| AppConfig::default().mixed_port);
        Self {
            title: language.tr(Message::CustomPort).to_string(),
            fields: vec![InputField {
                label: language.tr(Message::MixedPortLabel),
                placeholder: "7890",
                value: port.to_string(),
            }],
            focus: 0,
            submit: FormSubmit::SetMixedPort,
            message: language.tr(Message::PortPrompt).to_string(),
            language,
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
                    self.message = self
                        .language
                        .tr(Message::SubscriptionUrlRequired)
                        .to_string();
                    return None;
                }

                let name = self.fields[1].value.trim().to_string();
                let mut args = vec!["subscribe".to_string(), "add".to_string(), url];
                if !name.is_empty() {
                    args.push(name);
                }
                Some(CommandRequest::new(
                    self.language.tr(Message::AddSubscription),
                    args,
                ))
            }
            FormSubmit::AddLocalProfile => {
                let name = self.fields[0].value.trim().to_string();
                let config = self.fields[1].value.trim().to_string();
                if name.is_empty() {
                    self.message = self.language.tr(Message::ProfileNameRequired).to_string();
                    return None;
                }
                if config.is_empty() {
                    self.message = self.language.tr(Message::ConfigPathRequired).to_string();
                    return None;
                }

                Some(CommandRequest::new(
                    self.language.tr(Message::AddLocalProfile),
                    vec!["profile".to_string(), "add".to_string(), name, config],
                ))
            }
            FormSubmit::SetMixedPort => None,
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
    fn load(paths: &StarailPaths, language: Language) -> Self {
        let modes = vec!["rule", "global", "direct"];
        let (current, message) = match controller::get_configs(paths) {
            Ok(configs) => {
                let mode = configs
                    .get("mode")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned);
                (mode, language.tr(Message::ControllerModeLoaded).to_string())
            }
            Err(_) => (
                None,
                language.tr(Message::ControllerUnreachableRetry).to_string(),
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
    fn logs(paths: &StarailPaths, lines: usize, language: Language) -> Self {
        match tail_file(&paths.log_file, lines) {
            Ok(text) if !text.trim().is_empty() => Self {
                title: format!(
                    "{}: {}",
                    language.tr(Message::Logs),
                    paths.log_file.display()
                ),
                lines: text.lines().map(ToOwned::to_owned).collect(),
                scroll: 0,
                message: language.tr(Message::LogsLoaded).to_string(),
            },
            Ok(_) => Self {
                title: format!(
                    "{}: {}",
                    language.tr(Message::Logs),
                    paths.log_file.display()
                ),
                lines: vec![language.tr(Message::LogFileEmpty).to_string()],
                scroll: 0,
                message: language.tr(Message::LogsLoaded).to_string(),
            },
            Err(error) => Self {
                title: language.tr(Message::Logs).to_string(),
                lines: vec![format!("{error:#}")],
                scroll: 0,
                message: language.tr(Message::UnableReadLogs).to_string(),
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
    fn label(&self, language: Language) -> String {
        let kind = empty_as_dash(&self.kind);
        format!(
            "{} {} {} {}",
            pad_display_width(&self.name, 30),
            pad_display_width(&kind, 12),
            self.children.len(),
            language.tr(Message::NodeUnit)
        )
    }
}

struct ProxyPage {
    items: Vec<ProxyItem>,
    selected: usize,
    message: String,
}

impl ProxyPage {
    fn load(paths: &StarailPaths, language: Language) -> Result<Self> {
        let config = AppConfig::load(paths)?;
        let active = config
            .active_profile
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!(language.tr(Message::NoActiveProfileSelected)))?;
        let profile_path = paths.profile_config_path(active);
        let text = fs::read_to_string(&profile_path).with_context(|| {
            format!(
                "{} {}",
                language.tr(Message::FailedReadActiveProfile),
                profile_path.display()
            )
        })?;
        let yaml = serde_yaml::from_str::<serde_yaml::Value>(&text).with_context(|| {
            format!(
                "{} {}",
                language.tr(Message::FailedParseActiveProfile),
                profile_path.display()
            )
        })?;
        Ok(Self {
            items: proxy_groups_from_yaml(&yaml),
            selected: 0,
            message: format!("{} '{active}'.", language.tr(Message::LoadedProxyGroups)),
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
    fn display(&self, language: Language) -> String {
        match self {
            Self::Untested => "-".to_string(),
            Self::Testing => language.tr(Message::Testing).to_string(),
            Self::Ok(delay) => delay.clone(),
            Self::Failed => language.tr(Message::Failed).to_string(),
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

    fn node_label(&self, index: usize, node: &str, language: Language) -> String {
        let status = self
            .node_results
            .get(index)
            .map(|status| status.display(language))
            .unwrap_or_else(|| "-".to_string());
        let node = truncate_for_column(node, 44);
        format!(
            "{} {}",
            pad_display_width(&node, 44),
            pad_left_display_width(&status, 12)
        )
    }

    fn selected_status(&self, language: Language) -> String {
        self.node_results
            .get(self.selected)
            .map(|status| status.display(language))
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
    fn running(
        title: &str,
        args: &[String],
        elapsed: Duration,
        preview: Vec<String>,
        language: Language,
    ) -> Self {
        let mut lines = vec![
            format!(
                "{} starail {}",
                language.tr(Message::CommandLabel),
                args.join(" ")
            ),
            format!(
                "{} {}s",
                language.tr(Message::ElapsedLabel),
                elapsed.as_secs()
            ),
            format!(
                "{} {}",
                language.tr(Message::StatusLabel),
                language.tr(Message::StillRunning)
            ),
            language.tr(Message::PressCtrlCCancelCommand).to_string(),
        ];
        lines.extend(running_hint(args, language));
        if !preview.is_empty() {
            lines.push(String::new());
            lines.push(language.tr(Message::RecentOutput).to_string());
            lines.extend(preview);
        }

        Self {
            title: title.to_string(),
            lines,
            scroll: 0,
            success: None,
            message: language.tr(Message::CommandRunning).to_string(),
        }
    }

    fn error(title: &str, message: String, language: Language) -> Self {
        Self {
            title: title.to_string(),
            lines: vec![message],
            scroll: 0,
            success: Some(false),
            message: language.tr(Message::CommandFailed).to_string(),
        }
    }

    fn from_command(
        title: String,
        args: Vec<String>,
        result: io::Result<std::process::Output>,
        language: Language,
    ) -> Self {
        match result {
            Ok(output) => {
                let mut lines = command_output_lines(&args, &output, language);
                if lines.is_empty() {
                    lines.push(language.tr(Message::CommandCompletedNoOutput).to_string());
                }
                Self {
                    title,
                    lines,
                    scroll: 0,
                    success: Some(output.status.success()),
                    message: if output.status.success() {
                        language.tr(Message::CommandCompleted).to_string()
                    } else {
                        language.tr(Message::CommandFailed).to_string()
                    },
                }
            }
            Err(error) => {
                let message = if error.kind() == io::ErrorKind::Interrupted {
                    language.tr(Message::CommandCanceledByUser).to_string()
                } else {
                    format!("{}: {error}", language.tr(Message::FailedRunStarail))
                };
                Self {
                    title,
                    lines: vec![message],
                    scroll: 0,
                    success: Some(false),
                    message: language.tr(Message::CommandFailed).to_string(),
                }
            }
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
    Refresh,
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
            app.language.tr(Message::InstallMihomoCore),
            format!(
                "{} {}?",
                app.language.tr(Message::MihomoCoreMissingInstallQuestion),
                paths.core_file.display()
            ),
            app.language.tr(Message::Install),
            app.language.tr(Message::Later),
            CommandRequest::static_args(
                app.language.tr(Message::InstallMihomoCore),
                &["core", "install"],
            ),
            app.language.tr(Message::CoreInstallSkipped),
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
                    Effect::Refresh => app.refresh(paths),
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
        Screen::Settings(page) => handle_settings_key(paths, page, key, app.language),
        Screen::Language(page) => handle_language_key(paths, page, key),
        Screen::Profiles(page) => handle_profiles_key(paths, page, key, app.language),
        Screen::Form(form) => handle_form_key(paths, form, key),
        Screen::Mode(page) => handle_mode_key(page, key, app.language),
        Screen::Logs(page) => handle_text_key(page, key, TextPage::logs(paths, 200, app.language)),
        Screen::Proxies(page) => handle_proxies_key(paths, page, key, app.language),
        Screen::ProxyGroup(page) => handle_proxy_group_key(page, key, app.language),
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
            app.message = app.language.tr(Message::Refreshed).to_string();
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
        KeyCode::Enter => action_effect(paths, app.selected_action(), app.language),
        _ => (Screen::Home, Effect::None),
    }
}

fn action_effect(paths: &StarailPaths, action: Action, language: Language) -> (Screen, Effect) {
    match action {
        Action::StartStop => {
            if runtime::is_running(paths) {
                (
                    Screen::Home,
                    Effect::Run(CommandRequest::static_args(
                        language.tr(Message::StopMihomo),
                        &["stop"],
                    )),
                )
            } else {
                (
                    Screen::Home,
                    Effect::Run(CommandRequest::static_args(
                        language.tr(Message::StartMihomo),
                        &["start"],
                    )),
                )
            }
        }
        Action::Profiles => (profiles_screen(paths, language), Effect::None),
        Action::ListProxies => (proxies_screen(paths, language), Effect::None),
        Action::ToggleShellProxy => {
            if system_proxy::block_present(paths) {
                (
                    Screen::Confirm(ConfirmPage::new(
                        language.tr(Message::DisableShellProxy),
                        language.tr(Message::DisableShellProxyQuestion),
                        language.tr(Message::Disable),
                        language.tr(Message::Cancel),
                        CommandRequest::static_args(
                            language.tr(Message::DisableShellProxy),
                            &["system-proxy", "off"],
                        ),
                        language.tr(Message::NoChange),
                    )),
                    Effect::None,
                )
            } else {
                (
                    Screen::Confirm(ConfirmPage::new(
                        language.tr(Message::EnableShellProxy),
                        language.tr(Message::EnableShellProxyQuestion),
                        language.tr(Message::Enable),
                        language.tr(Message::Cancel),
                        CommandRequest::static_args(
                            language.tr(Message::EnableShellProxy),
                            &["system-proxy", "on"],
                        ),
                        language.tr(Message::NoChange),
                    )),
                    Effect::None,
                )
            }
        }
        Action::Settings => (Screen::Settings(SettingsPage::load(language)), Effect::None),
    }
}

fn handle_settings_key(
    paths: &StarailPaths,
    mut page: SettingsPage,
    key: KeyEvent,
    language: Language,
) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => (Screen::Home, Effect::None),
        KeyCode::Down | KeyCode::Char('j') => {
            page.next();
            (Screen::Settings(page), Effect::None)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            page.previous();
            (Screen::Settings(page), Effect::None)
        }
        KeyCode::Enter => match page.selected_item() {
            Some(SettingsItemKind::Language) => {
                (Screen::Language(LanguagePage::load(language)), Effect::None)
            }
            Some(SettingsItemKind::MixedPort) => (
                Screen::Form(InputForm::mixed_port(paths, language)),
                Effect::None,
            ),
            Some(SettingsItemKind::SwitchMode) => {
                (Screen::Mode(ModePage::load(paths, language)), Effect::None)
            }
            Some(SettingsItemKind::Logs) => (
                Screen::Logs(TextPage::logs(paths, 200, language)),
                Effect::None,
            ),
            Some(SettingsItemKind::CheckUpdateCore) => check_update_core_confirm(language),
            None => (Screen::Settings(page), Effect::None),
        },
        _ => (Screen::Settings(page), Effect::None),
    }
}

fn check_update_core_confirm(language: Language) -> (Screen, Effect) {
    (
        Screen::Confirm(ConfirmPage::new(
            language.tr(Message::CheckUpdateCore),
            language.tr(Message::CheckUpdateCoreQuestion),
            language.tr(Message::CheckAction),
            language.tr(Message::Cancel),
            CommandRequest::static_args(
                language.tr(Message::CheckUpdateCore),
                &["core", "install"],
            ),
            language.tr(Message::NoChange),
        )),
        Effect::None,
    )
}

fn settings_screen_with_message(
    message: String,
    language: Language,
    selected_item: SettingsItemKind,
) -> Screen {
    let mut page = SettingsPage::load(language);
    if let Some(index) = page.items.iter().position(|item| *item == selected_item) {
        page.selected = index;
    }
    page.message = message;
    Screen::Settings(page)
}

fn handle_language_key(
    paths: &StarailPaths,
    mut page: LanguagePage,
    key: KeyEvent,
) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => (
            Screen::Settings(SettingsPage::load(page.current)),
            Effect::None,
        ),
        KeyCode::Down | KeyCode::Char('j') => {
            page.next();
            (Screen::Language(page), Effect::None)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            page.previous();
            (Screen::Language(page), Effect::None)
        }
        KeyCode::Enter => {
            let selected = page.selected_language();
            match save_language(paths, selected) {
                Ok(()) => (
                    settings_screen_with_message(
                        selected.tr(Message::LanguageSaved).to_string(),
                        selected,
                        SettingsItemKind::Language,
                    ),
                    Effect::Refresh,
                ),
                Err(error) => {
                    page.message = format!("{error:#}");
                    (Screen::Language(page), Effect::None)
                }
            }
        }
        _ => (Screen::Language(page), Effect::None),
    }
}

fn save_language(paths: &StarailPaths, language: Language) -> Result<()> {
    let mut config = AppConfig::load(paths)?;
    config.language = Some(language.code().to_string());
    config.save(paths)
}

fn handle_profiles_key(
    paths: &StarailPaths,
    mut page: ProfilePage,
    key: KeyEvent,
    language: Language,
) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => (Screen::Home, Effect::None),
        KeyCode::Char('r') => (profiles_screen(paths, language), Effect::None),
        KeyCode::Down | KeyCode::Char('j') => {
            page.next();
            (Screen::Profiles(page), Effect::None)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            page.previous();
            (Screen::Profiles(page), Effect::None)
        }
        KeyCode::Char('a') => (
            Screen::Form(InputForm::local_profile(language)),
            Effect::None,
        ),
        KeyCode::Char('s') => (
            Screen::Form(InputForm::subscription(language)),
            Effect::None,
        ),
        KeyCode::Char('U') => (
            Screen::Profiles(page),
            Effect::Run(CommandRequest::static_args(
                language.tr(Message::UpdateSubscriptions),
                &["subscribe", "update"],
            )),
        ),
        KeyCode::Char('u') => {
            let Some(selected) = page.selected_profile() else {
                page.message = language.tr(Message::NoProfileSelected).to_string();
                return (Screen::Profiles(page), Effect::None);
            };
            if selected.kind != profile::ProfileKind::Subscription {
                page.message = language
                    .tr(Message::LocalProfileUpdateUnsupported)
                    .to_string();
                return (Screen::Profiles(page), Effect::None);
            }
            let selected_name = selected.name.clone();
            (
                Screen::Profiles(page),
                Effect::Run(CommandRequest::new(
                    language.tr(Message::UpdateSubscription),
                    vec!["subscribe".to_string(), "update".to_string(), selected_name],
                )),
            )
        }
        KeyCode::Enter => {
            let Some(selected) = page.selected_profile() else {
                page.message = language.tr(Message::NoProfileSelected).to_string();
                return (Screen::Profiles(page), Effect::None);
            };
            let selected_name = selected.name.clone();
            (
                Screen::Profiles(page),
                Effect::Run(CommandRequest::new(
                    language.tr(Message::UseProfile),
                    vec!["profile".to_string(), "use".to_string(), selected_name],
                )),
            )
        }
        KeyCode::Char('d') => {
            let Some(selected) = page.selected_profile() else {
                page.message = language.tr(Message::NoProfileSelected).to_string();
                return (Screen::Profiles(page), Effect::None);
            };
            let selected_name = selected.name.clone();
            (
                Screen::Confirm(ConfirmPage::new(
                    language.tr(Message::RemoveProfile),
                    format!(
                        "{} '{}' ?",
                        language.tr(Message::RemoveProfileQuestion),
                        selected_name
                    ),
                    language.tr(Message::Remove),
                    language.tr(Message::Cancel),
                    CommandRequest::new(
                        language.tr(Message::RemoveProfile),
                        vec!["profile".to_string(), "remove".to_string(), selected_name],
                    ),
                    language.tr(Message::NoChange),
                )),
                Effect::None,
            )
        }
        _ => (Screen::Profiles(page), Effect::None),
    }
}

fn handle_form_key(paths: &StarailPaths, mut form: InputForm, key: KeyEvent) -> (Screen, Effect) {
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

            if form.submit == FormSubmit::SetMixedPort {
                let value = form.fields[0].value.trim().to_string();
                return match save_mixed_port(paths, &value) {
                    Ok(port) => (
                        settings_screen_with_message(
                            format!(
                                "{} ({port})",
                                app_language(paths).tr(Message::MixedPortSaved)
                            ),
                            app_language(paths),
                            SettingsItemKind::MixedPort,
                        ),
                        Effect::Refresh,
                    ),
                    Err(error) => {
                        form.message = format!("{error:#}");
                        (Screen::Form(form), Effect::None)
                    }
                };
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

fn save_mixed_port(paths: &StarailPaths, value: &str) -> Result<u16> {
    let port = parse_mixed_port(value)?;
    let mut config = AppConfig::load(paths)?;
    config.mixed_port = port;
    config.save(paths)?;
    Ok(port)
}

fn parse_mixed_port(value: &str) -> Result<u16> {
    let port = value
        .trim()
        .parse::<u16>()
        .map_err(|_| anyhow::anyhow!(i18n::detect().tr(Message::PortInvalid)))?;
    if port == 0 {
        return Err(anyhow::anyhow!(i18n::detect().tr(Message::PortInvalid)));
    }
    Ok(port)
}

fn handle_mode_key(mut page: ModePage, key: KeyEvent, language: Language) -> (Screen, Effect) {
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
                    language.tr(Message::SwitchMode),
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
    language: Language,
) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => (Screen::Home, Effect::None),
        KeyCode::Char('r') => (proxies_screen(paths, language), Effect::None),
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
                page.message = language.tr(Message::NoProxyGroupSelected).to_string();
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
                    message: language.tr(Message::NodesLoadedYamlOrder).to_string(),
                }),
                Effect::None,
            )
        }
        _ => (Screen::Proxies(page), Effect::None),
    }
}

fn handle_proxy_group_key(
    mut page: ProxyGroupPage,
    key: KeyEvent,
    language: Language,
) -> (Screen, Effect) {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace => {
            let selected = page
                .back_selected
                .min(page.back_items.len().saturating_sub(1));
            (
                Screen::Proxies(ProxyPage {
                    items: page.back_items,
                    selected,
                    message: language.tr(Message::ProxyDataLoaded).to_string(),
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
                page.message = language.tr(Message::GroupNoNodesTest).to_string();
                (Screen::ProxyGroup(page), Effect::None)
            } else {
                let next = page.clone();
                (Screen::ProxyGroup(page), Effect::TestProxyGroup(next))
            }
        }
        KeyCode::Enter => {
            let Some(node) = page.selected_node() else {
                page.message = language.tr(Message::NoNodeSelected).to_string();
                return (Screen::ProxyGroup(page), Effect::None);
            };
            let group_name = page.group.name.clone();
            let node_name = node.to_string();
            (
                Screen::ProxyGroup(page),
                Effect::Run(CommandRequest::new(
                    language.tr(Message::SelectProxyNode),
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
        app.language,
    ));
    terminal.draw(|frame| draw(frame, app))?;

    let result = run_child_command(terminal, app, paths, &request);
    app.refresh(paths);
    app.screen = Screen::Output(OutputPage::from_command(
        request.title,
        request.args,
        result,
        app.language,
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
        .env("STARAIL_LANG", app.language.code())
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
            app.language,
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
        page.message = app.language.tr(Message::GroupNoNodesTest).to_string();
        app.screen = Screen::ProxyGroup(page);
        return Ok(());
    }

    let app_config = AppConfig::load(paths)?;
    let total = page.group.children.len();
    page.node_results = vec![NodeTestStatus::Testing; total];
    page.message = proxy_test_start_message(
        app.language,
        total,
        &app_config.latency_test_url,
        app_config.latency_test_timeout,
    );
    app.screen = Screen::ProxyGroup(page.clone());
    terminal.draw(|frame| draw(frame, app))?;

    let (sender, receiver) = mpsc::channel();
    for (index, node) in page.group.children.iter().cloned().enumerate() {
        let sender = sender.clone();
        let paths = paths.clone();
        let test_url = app_config.latency_test_url.clone();
        let timeout = app_config.latency_test_timeout;
        let language = app.language;
        thread::spawn(move || {
            let result = controller::delay(&paths, &node, &test_url, timeout)
                .map(|value| delay_label(value, language))
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
                page.message = proxy_test_progress_message(app.language, finished, total, ok);
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
                    page.message = proxy_test_cancelled_message(app.language);
                    app.screen = Screen::ProxyGroup(page);
                    return Ok(());
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }

    clear_testing_statuses(&mut page);
    page.message = proxy_test_finished_message(app.language, finished, total, ok);
    app.screen = Screen::ProxyGroup(page);
    Ok(())
}

fn delay_label(value: Value, language: Language) -> String {
    value
        .get("delay")
        .and_then(Value::as_u64)
        .map(|value| format!("{value}ms"))
        .unwrap_or_else(|| language.tr(Message::OkStatus).to_string())
}

fn proxy_test_start_message(
    language: Language,
    total: usize,
    test_url: &str,
    timeout: u64,
) -> String {
    match language {
        Language::English => format!(
            "Testing {total} nodes concurrently with {test_url} timeout={timeout}ms. Press Esc or Ctrl-C to cancel."
        ),
        Language::SimplifiedChinese => format!(
            "正在并发测试 {total} 个节点，测试地址 {test_url}，超时 {timeout}ms。按 Esc 或 Ctrl-C 取消。"
        ),
    }
}

fn proxy_test_progress_message(
    language: Language,
    finished: usize,
    total: usize,
    ok: usize,
) -> String {
    match language {
        Language::English => {
            format!("Testing nodes... {finished}/{total} finished, {ok} ok.")
        }
        Language::SimplifiedChinese => {
            format!("正在测试节点... 已完成 {finished}/{total}，成功 {ok}。")
        }
    }
}

fn proxy_test_cancelled_message(language: Language) -> String {
    match language {
        Language::English => {
            "Test cancelled; in-flight node checks may finish in background.".to_string()
        }
        Language::SimplifiedChinese => {
            "测试已取消；正在进行的节点检查可能会在后台完成。".to_string()
        }
    }
}

fn proxy_test_finished_message(
    language: Language,
    finished: usize,
    total: usize,
    ok: usize,
) -> String {
    match language {
        Language::English if ok == 0 => {
            format!("Finished testing {finished}/{total} nodes; all failed.")
        }
        Language::English => format!("Finished testing {finished}/{total} nodes; {ok} ok."),
        Language::SimplifiedChinese if ok == 0 => {
            format!("测试完成 {finished}/{total}；全部失败。")
        }
        Language::SimplifiedChinese => format!("测试完成 {finished}/{total}；成功 {ok}。"),
    }
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

    draw_header(
        frame,
        chunks[0],
        app,
        &screen_title(&app.screen, app.language),
    );
    match &app.screen {
        Screen::Home => draw_home(frame, chunks[1], app),
        Screen::Settings(page) => draw_settings(frame, chunks[1], page, app.language),
        Screen::Language(page) => draw_language(frame, chunks[1], page, app.language),
        Screen::Profiles(page) => draw_profiles(frame, chunks[1], page, app.language),
        Screen::Form(form) => draw_form(frame, chunks[1], form),
        Screen::Mode(page) => draw_mode(frame, chunks[1], page, app.language),
        Screen::Logs(page) => draw_text_page(frame, chunks[1], page),
        Screen::Proxies(page) => draw_proxies(frame, chunks[1], page, app.language),
        Screen::ProxyGroup(page) => draw_proxy_group(frame, chunks[1], page, app.language),
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
    spans.extend(header_status_spans(app.status.as_ref(), app.language));
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
            footer_message_line(content.message, app.language),
            footer_hint_line(&content.hints),
        ])
        .block(
            Block::default()
                .title(app.language.tr(Message::MessageKeys))
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

fn footer_message_line(message: String, language: Language) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            language.tr(Message::StatusLabel),
            Style::default().fg(Color::DarkGray),
        ),
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

fn header_status_spans(
    snapshot: Option<&status::StatusSnapshot>,
    language: Language,
) -> Vec<Span<'static>> {
    match snapshot {
        Some(status) => vec![
            Span::styled(
                format!("{}: ", language.tr(Message::Process).to_lowercase()),
                status_label_style(),
            ),
            Span::styled(
                status_display_value(&status.process, language),
                status_value_style(process_tone(&status.process)),
            ),
            Span::styled(
                format!(" | {}: ", language.tr(Message::Active)),
                status_label_style(),
            ),
            Span::styled(
                status_display_value(&status.active_profile, language),
                status_value_style(profile_tone(&status.active_profile)),
            ),
        ],
        None => vec![Span::styled(
            language.tr(Message::Unavailable),
            status_value_style(StatusTone::Problem),
        )],
    }
}

fn home_status_lines(status: &status::StatusSnapshot, language: Language) -> Vec<Line<'static>> {
    vec![
        status_line(
            language.tr(Message::Home),
            status.home.clone(),
            StatusTone::Neutral,
        ),
        status_line(
            language.tr(Message::Core),
            status_display_value(&status.core, language),
            core_tone(&status.core),
        ),
        status_line(
            language.tr(Message::CoreVersion),
            status_display_value(&status.core_version, language),
            version_tone(&status.core_version),
        ),
        status_line(
            language.tr(Message::ActiveProfile),
            status_display_value(&status.active_profile, language),
            profile_tone(&status.active_profile),
        ),
        status_line(
            language.tr(Message::CurrentGroupNode),
            status_display_value(&status.selected_proxy, language),
            selected_proxy_tone(&status.selected_proxy),
        ),
        status_line(
            language.tr(Message::MixedPort),
            status.mixed_port.to_string(),
            StatusTone::Normal,
        ),
        status_line(
            language.tr(Message::Controller),
            status.controller.clone(),
            StatusTone::Neutral,
        ),
        status_line(
            language.tr(Message::Process),
            status_display_value(&status.process, language),
            process_tone(&status.process),
        ),
        status_line(
            language.tr(Message::ControllerState),
            status_display_value(&status.controller_state, language),
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

fn status_display_value(value: &str, language: Language) -> String {
    if let Some(pid) = value
        .strip_prefix("running (pid ")
        .and_then(|value| value.strip_suffix(')'))
    {
        return format!("{} (pid {pid})", language.tr(Message::Running));
    }

    if let Some(mode) = value
        .strip_prefix("reachable (mode: ")
        .and_then(|value| value.strip_suffix(')'))
    {
        return format!(
            "{} ({}: {mode})",
            language.tr(Message::Reachable),
            language.tr(Message::Mode)
        );
    }

    if let Some((prefix, rest)) = value.rsplit_once(" (+") {
        if let Some(count) = rest.strip_suffix(" groups)") {
            return format!(
                "{} (+{} {})",
                prefix,
                count,
                language.tr(Message::GroupUnit)
            );
        }
    }

    match value {
        "missing" => language.tr(Message::Missing).to_string(),
        "unavailable" => language.tr(Message::Unavailable).to_string(),
        "unknown" => language.tr(Message::Unknown).to_string(),
        "none" => language.tr(Message::NoneValue).to_string(),
        "stopped" => language.tr(Message::Stopped).to_string(),
        "reachable" => language.tr(Message::Reachable).to_string(),
        "unreachable" => language.tr(Message::Unreachable).to_string(),
        _ => value.to_string(),
    }
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
        Some(status) => home_status_lines(status, app.language),
        None => vec![Line::from(Span::styled(
            app.language.tr(Message::Unavailable),
            status_value_style(StatusTone::Problem),
        ))],
    };
    let status = Paragraph::new(status_lines)
        .wrap(Wrap { trim: false })
        .block(page_block(app.language.tr(Message::Status)));
    frame.render_widget(status, body[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(5)])
        .split(body[1]);

    let items = app
        .actions
        .iter()
        .map(|action| {
            ListItem::new(format!(
                "{} {}",
                pad_display_width(action.label(app.language), 24),
                action.detail(app.language)
            ))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    state.select(Some(app.selected));
    let actions = List::new(items)
        .block(page_block(app.language.tr(Message::DailyActions)))
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
            selected.label(app.language),
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(selected.detail(app.language)),
    ])
    .wrap(Wrap { trim: false })
    .block(page_block(app.language.tr(Message::Selected)));
    frame.render_widget(detail, right[1]);
}

fn draw_language(frame: &mut Frame<'_>, area: Rect, page: &LanguagePage, language: Language) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(area);

    let items = page
        .options
        .iter()
        .map(|candidate| {
            let active = if *candidate == page.current { "*" } else { " " };
            ListItem::new(format!(
                "{} {} {}",
                active,
                pad_display_width(candidate.display_name(language), 20),
                candidate.code()
            ))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    if !page.options.is_empty() {
        state.select(Some(page.selected));
    }
    let list = List::new(items)
        .block(page_block(language.tr(Message::Language)))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, body[0], &mut state);

    let selected = page.selected_language();
    let detail = Paragraph::new(vec![
        setting_detail_line(
            language.tr(Message::Current),
            page.current.display_name(language).to_string(),
        ),
        setting_detail_line(
            language.tr(Message::Selected),
            selected.display_name(language).to_string(),
        ),
        setting_detail_line(language.tr(Message::Code), selected.code().to_string()),
        Line::from(language.tr(Message::LanguageDetail)),
    ])
    .wrap(Wrap { trim: false })
    .block(page_block(language.tr(Message::Details)));
    frame.render_widget(detail, body[1]);
}

fn draw_settings(frame: &mut Frame<'_>, area: Rect, page: &SettingsPage, language: Language) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(area);

    let items = page
        .items
        .iter()
        .map(|item| {
            ListItem::new(format!(
                "{} {}",
                pad_display_width(item.label(language), 24),
                item.summary(language)
            ))
        })
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    if !page.items.is_empty() {
        state.select(Some(page.selected));
    }
    let list = List::new(items)
        .block(page_block(language.tr(Message::Settings)))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, body[0], &mut state);

    let detail_lines = page
        .selected_item()
        .map(|item| item.detail_lines(language))
        .unwrap_or_else(|| vec![Line::from(language.tr(Message::SettingsNoItems))]);
    let detail = Paragraph::new(detail_lines)
        .wrap(Wrap { trim: false })
        .block(page_block(language.tr(Message::Details)));
    frame.render_widget(detail, body[1]);
}

fn setting_detail_line(label: &'static str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), Style::default().fg(Color::Gray)),
        Span::raw(value),
    ])
}

fn label_prefix(label: &'static str) -> String {
    format!("{label}: ")
}

fn profile_kind_label(kind: profile::ProfileKind, language: Language) -> &'static str {
    match kind {
        profile::ProfileKind::Local => language.tr(Message::Local),
        profile::ProfileKind::Subscription => language.tr(Message::Subscription),
    }
}

fn draw_profiles(frame: &mut Frame<'_>, area: Rect, page: &ProfilePage, language: Language) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(area);

    let items = if page.profiles.is_empty() {
        vec![ListItem::new(language.tr(Message::NoProfilesYet))]
    } else {
        page.profiles
            .iter()
            .map(|profile| {
                let active = if profile.active { "*" } else { " " };
                ListItem::new(format!(
                    "{} {} {}",
                    active,
                    pad_display_width(&profile.name, 30),
                    pad_display_width(profile_kind_label(profile.kind, language), 12)
                ))
            })
            .collect::<Vec<_>>()
    };
    let mut state = ListState::default();
    if !page.profiles.is_empty() {
        state.select(Some(page.selected));
    }
    let list = List::new(items)
        .block(page_block(language.tr(Message::Profiles)))
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
                Span::styled(
                    label_prefix(language.tr(Message::Name)),
                    Style::default().fg(Color::Gray),
                ),
                Span::raw(profile.name.clone()),
            ]),
            Line::from(vec![
                Span::styled(
                    label_prefix(language.tr(Message::Type)),
                    Style::default().fg(Color::Gray),
                ),
                Span::raw(profile_kind_label(profile.kind, language).to_string()),
            ]),
            Line::from(vec![
                Span::styled(
                    label_prefix(language.tr(Message::Active)),
                    Style::default().fg(Color::Gray),
                ),
                Span::raw(if profile.active {
                    language.tr(Message::Yes)
                } else {
                    language.tr(Message::No)
                }),
            ]),
            Line::from(vec![
                Span::styled(
                    label_prefix(language.tr(Message::Source)),
                    Style::default().fg(Color::Gray),
                ),
                Span::raw(profile.source_url.as_deref().unwrap_or("-").to_string()),
            ]),
        ],
        None => vec![
            Line::from(language.tr(Message::ProfileEmptyHint)),
            Line::from(language.tr(Message::ProfileStorageHint)),
        ],
    };
    let detail = Paragraph::new(detail_lines)
        .wrap(Wrap { trim: false })
        .block(page_block(language.tr(Message::Details)));
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

fn draw_mode(frame: &mut Frame<'_>, area: Rect, page: &ModePage, language: Language) {
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
        .block(page_block(language.tr(Message::Mode)))
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
            Span::styled(
                label_prefix(language.tr(Message::Current)),
                Style::default().fg(Color::Gray),
            ),
            Span::raw(match page.current.as_deref() {
                Some(mode) => mode.to_string(),
                None => language.tr(Message::Unknown).to_string(),
            }),
        ]),
        Line::from(vec![
            Span::styled(
                label_prefix(language.tr(Message::Selected)),
                Style::default().fg(Color::Gray),
            ),
            Span::raw(page.selected_mode()),
        ]),
        Line::from(page.message.clone()),
    ])
    .wrap(Wrap { trim: false })
    .block(page_block(language.tr(Message::Controller)));
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

fn draw_proxies(frame: &mut Frame<'_>, area: Rect, page: &ProxyPage, language: Language) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    let items = if page.items.is_empty() {
        vec![ListItem::new(language.tr(Message::NoProxyGroupsFound))]
    } else {
        page.items
            .iter()
            .map(|item| ListItem::new(item.label(language)))
            .collect::<Vec<_>>()
    };
    let mut state = ListState::default();
    if !page.items.is_empty() {
        state.select(Some(page.selected));
    }
    let list = List::new(items)
        .block(page_block(language.tr(Message::ProxyGroups)))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    frame.render_stateful_widget(list, body[0], &mut state);

    let lines = match page.selected_item() {
        Some(item) => proxy_detail_lines(item, language),
        None => vec![Line::from(
            language.tr(Message::SelectActiveProfileWithProxyGroups),
        )],
    };
    let detail = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .block(page_block(language.tr(Message::Details)));
    frame.render_widget(detail, body[1]);
}

fn draw_proxy_group(frame: &mut Frame<'_>, area: Rect, page: &ProxyGroupPage, language: Language) {
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(58), Constraint::Percentage(42)])
        .split(area);
    let items = page
        .group
        .children
        .iter()
        .enumerate()
        .map(|(index, node)| ListItem::new(page.node_label(index, node, language)))
        .collect::<Vec<_>>();
    let mut state = ListState::default();
    if !page.group.children.is_empty() {
        state.select(Some(page.selected));
    }
    let group_title = format!("{}: {}", language.tr(Message::Group), page.group.name);
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
            Span::styled(
                label_prefix(language.tr(Message::Type)),
                Style::default().fg(Color::Gray),
            ),
            Span::raw(empty_as_dash(&page.group.kind)),
        ]),
        Line::from(vec![
            Span::styled(
                label_prefix(language.tr(Message::Selected)),
                Style::default().fg(Color::Gray),
            ),
            Span::raw(page.selected_node().unwrap_or("-").to_string()),
        ]),
        Line::from(vec![
            Span::styled(
                label_prefix(language.tr(Message::Result)),
                Style::default().fg(Color::Gray),
            ),
            Span::raw(page.selected_status(language)),
        ]),
        Line::from(page.message.clone()),
    ])
    .wrap(Wrap { trim: false })
    .block(page_block(language.tr(Message::Selection)));
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

fn profiles_screen(paths: &StarailPaths, language: Language) -> Screen {
    match ProfilePage::load(paths, language) {
        Ok(page) => Screen::Profiles(page),
        Err(error) => Screen::Output(OutputPage::error(
            language.tr(Message::Profiles),
            format!("{error:#}"),
            language,
        )),
    }
}

fn proxies_screen(paths: &StarailPaths, language: Language) -> Screen {
    match ProxyPage::load(paths, language) {
        Ok(page) => Screen::Proxies(page),
        Err(error) => Screen::Output(OutputPage::error(
            language.tr(Message::ProxyGroupsAndNodes),
            format!("{error:#}"),
            language,
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

fn proxy_detail_lines(item: &ProxyItem, language: Language) -> Vec<Line<'static>> {
    vec![
        Line::from(vec![
            Span::styled(
                label_prefix(language.tr(Message::Group)),
                Style::default().fg(Color::Gray),
            ),
            Span::raw(item.name.clone()),
        ]),
        Line::from(vec![
            Span::styled(
                label_prefix(language.tr(Message::Type)),
                Style::default().fg(Color::Gray),
            ),
            Span::raw(empty_as_dash(&item.kind)),
        ]),
        Line::from(vec![
            Span::styled(
                label_prefix(language.tr(Message::Nodes)),
                Style::default().fg(Color::Gray),
            ),
            Span::raw(item.children.len().to_string()),
        ]),
        Line::from(language.tr(Message::PressEnterViewGroupNodeList)),
    ]
}

#[cfg(test)]
mod tui_tests {
    use super::*;

    #[test]
    fn settings_page_groups_language_display_mode_logs_and_core() {
        let page = SettingsPage::load(Language::English);

        assert_eq!(
            page.items,
            [
                SettingsItemKind::Language,
                SettingsItemKind::MixedPort,
                SettingsItemKind::SwitchMode,
                SettingsItemKind::Logs,
                SettingsItemKind::CheckUpdateCore,
            ]
        );
        assert_eq!(page.items[0].label(Language::English), "Language");
        assert_eq!(page.items[1].label(Language::English), "Custom port");
        assert_eq!(page.items[2].label(Language::English), "Switch mode");
        assert_eq!(page.items[3].label(Language::English), "Logs");
        assert_eq!(page.items[4].label(Language::English), "Check/update core");
    }

    #[test]
    fn parses_mixed_port_values() {
        assert_eq!(parse_mixed_port("7891").expect("port should parse"), 7891);
        assert!(parse_mixed_port("0").is_err());
        assert!(parse_mixed_port("65536").is_err());
        assert!(parse_mixed_port("abc").is_err());
    }

    #[test]
    fn pads_cjk_text_by_display_width() {
        let padded = pad_display_width("设置", 8);

        assert_eq!(UnicodeWidthStr::width("设置"), 4);
        assert_eq!(UnicodeWidthStr::width(padded.as_str()), 8);
        assert_eq!(padded, "设置    ");
    }

    #[test]
    fn truncates_cjk_text_by_display_width() {
        let truncated = truncate_for_column("节点节点节点", 7);

        assert_eq!(UnicodeWidthStr::width(truncated.as_str()), 7);
        assert_eq!(truncated, "节点...");
    }

    #[test]
    fn translates_status_values_for_cjk_ui() {
        let language = Language::SimplifiedChinese;

        assert_eq!(status_display_value("missing", language), "缺失");
        assert_eq!(status_display_value("none", language), "无");
        assert_eq!(
            status_display_value("running (pid 42)", language),
            "运行中 (pid 42)"
        );
        assert_eq!(
            status_display_value("reachable (mode: rule)", language),
            "可连接 (模式: rule)"
        );
        assert_eq!(
            status_display_value("Proxy -> Node (+2 groups)", language),
            "Proxy -> Node (+2 组)"
        );
    }

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

fn running_hint(args: &[String], language: Language) -> Vec<String> {
    if args.len() >= 2 && args[0] == "subscribe" && args[1] == "add" {
        vec![
            language.tr(Message::SubscriptionRunHintFetch).to_string(),
            language.tr(Message::SubscriptionRunHintTimeout).to_string(),
        ]
    } else if args.len() >= 2 && args[0] == "core" && args[1] == "install" {
        vec![language.tr(Message::CoreInstallRunHint).to_string()]
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

fn command_output_lines(
    args: &[String],
    output: &std::process::Output,
    language: Language,
) -> Vec<String> {
    let mut lines = Vec::new();
    if !output.status.success() {
        lines.push(format!(
            "{} starail {} ({})",
            language.tr(Message::CommandFailedLine),
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

fn screen_title(screen: &Screen, language: Language) -> String {
    match screen {
        Screen::Home => language.tr(Message::Dashboard).to_string(),
        Screen::Settings(_) => language.tr(Message::Settings).to_string(),
        Screen::Language(_) => language.tr(Message::Language).to_string(),
        Screen::Profiles(_) => language.tr(Message::Profiles).to_string(),
        Screen::Form(form) => form.title.clone(),
        Screen::Mode(_) => language.tr(Message::SwitchMode).to_string(),
        Screen::Logs(_) => language.tr(Message::Logs).to_string(),
        Screen::Proxies(_) => language.tr(Message::ProxyGroupsAndNodes).to_string(),
        Screen::ProxyGroup(page) => format!("{}: {}", language.tr(Message::Group), page.group.name),
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
                    label: app.language.tr(Message::Move),
                },
                FooterHint {
                    key: "Enter",
                    label: app.language.tr(Message::Open),
                },
                FooterHint {
                    key: "r",
                    label: app.language.tr(Message::Refresh),
                },
                FooterHint {
                    key: "q",
                    label: app.language.tr(Message::Quit),
                },
            ],
        },
        Screen::Settings(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: app.language.tr(Message::Move),
                },
                FooterHint {
                    key: "Enter",
                    label: app.language.tr(Message::Select),
                },
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Back),
                },
            ],
        },
        Screen::Language(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: app.language.tr(Message::Move),
                },
                FooterHint {
                    key: "Enter",
                    label: app.language.tr(Message::Apply),
                },
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Back),
                },
            ],
        },
        Screen::Profiles(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: app.language.tr(Message::Move),
                },
                FooterHint {
                    key: "Enter",
                    label: app.language.tr(Message::Use),
                },
                FooterHint {
                    key: "a",
                    label: app.language.tr(Message::Local),
                },
                FooterHint {
                    key: "s",
                    label: app.language.tr(Message::Subscription),
                },
                FooterHint {
                    key: "u",
                    label: app.language.tr(Message::Update),
                },
                FooterHint {
                    key: "U",
                    label: app.language.tr(Message::UpdateAll),
                },
                FooterHint {
                    key: "d",
                    label: app.language.tr(Message::Remove),
                },
                FooterHint {
                    key: "r",
                    label: app.language.tr(Message::Refresh),
                },
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Back),
                },
            ],
        },
        Screen::Form(form) => FooterContent {
            message: form.message.clone(),
            hints: vec![
                FooterHint {
                    key: "Tab",
                    label: app.language.tr(Message::Field),
                },
                FooterHint {
                    key: "Enter",
                    label: app.language.tr(Message::NextSubmit),
                },
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Cancel),
                },
            ],
        },
        Screen::Mode(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: app.language.tr(Message::Move),
                },
                FooterHint {
                    key: "Enter",
                    label: app.language.tr(Message::Apply),
                },
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Back),
                },
            ],
        },
        Screen::Logs(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: app.language.tr(Message::Scroll),
                },
                FooterHint {
                    key: "PgUp/PgDn",
                    label: app.language.tr(Message::Fast),
                },
                FooterHint {
                    key: "r",
                    label: app.language.tr(Message::Refresh),
                },
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Back),
                },
            ],
        },
        Screen::Proxies(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: app.language.tr(Message::Move),
                },
                FooterHint {
                    key: "Enter",
                    label: app.language.tr(Message::OpenGroup),
                },
                FooterHint {
                    key: "r",
                    label: app.language.tr(Message::Refresh),
                },
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Back),
                },
            ],
        },
        Screen::ProxyGroup(page) if page.is_testing() => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Cancel),
                },
                FooterHint {
                    key: "Ctrl-C",
                    label: app.language.tr(Message::Cancel),
                },
            ],
        },
        Screen::ProxyGroup(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: app.language.tr(Message::Move),
                },
                FooterHint {
                    key: "Enter",
                    label: app.language.tr(Message::SelectNode),
                },
                FooterHint {
                    key: "t",
                    label: app.language.tr(Message::TestGroup),
                },
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Back),
                },
            ],
        },
        Screen::Output(page) => FooterContent {
            message: page.message.clone(),
            hints: vec![
                FooterHint {
                    key: "j/k",
                    label: app.language.tr(Message::Scroll),
                },
                FooterHint {
                    key: "PgUp/PgDn",
                    label: app.language.tr(Message::Fast),
                },
                FooterHint {
                    key: "Enter",
                    label: app.language.tr(Message::Back),
                },
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Back),
                },
            ],
        },
        Screen::Confirm(_) => FooterContent {
            message: app.language.tr(Message::Choose).to_string(),
            hints: vec![
                FooterHint {
                    key: "Left/Right",
                    label: app.language.tr(Message::Choose),
                },
                FooterHint {
                    key: "Enter",
                    label: app.language.tr(Message::ConfirmAction),
                },
                FooterHint {
                    key: "Esc",
                    label: app.language.tr(Message::Cancel),
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

fn pad_display_width(value: &str, width: usize) -> String {
    let display_width = UnicodeWidthStr::width(value);
    if display_width >= width {
        value.to_string()
    } else {
        format!("{}{}", value, " ".repeat(width - display_width))
    }
}

fn pad_left_display_width(value: &str, width: usize) -> String {
    let display_width = UnicodeWidthStr::width(value);
    if display_width >= width {
        value.to_string()
    } else {
        format!("{}{}", " ".repeat(width - display_width), value)
    }
}

fn take_display_width(value: &str, width: usize) -> String {
    let mut output = String::new();
    let mut used = 0usize;
    for character in value.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width {
            break;
        }
        output.push(character);
        used += character_width;
    }
    output
}

fn truncate_for_column(value: &str, width: usize) -> String {
    if UnicodeWidthStr::width(value) <= width {
        return value.to_string();
    }

    if width <= 3 {
        return take_display_width(value, width);
    }

    let mut output = take_display_width(value, width - 3);
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
