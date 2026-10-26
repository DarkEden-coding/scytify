//! Settings page: appearance, account, developer app, storage and session.
use super::sidebar::thin_scrollbar;
use super::*;
use crate::frontend::layout::CONTENT_PADDING;
use iced::widget::column;
use iced::widget::{rule, scrollable, text_input};

/// Column of key combinations in the shortcut list.
const KEYS_WIDTH: f32 = 140.0;
/// Where users create the developer app whose Client ID the app needs.
const DEVELOPER_DASHBOARD: &str = "https://developer.spotify.com/dashboard";

impl Ui {
    /// Opens Spotify's developer dashboard, where the Client ID is created and shown.
    pub(super) fn dashboard_link(&self) -> Element<'_> {
        self.control(self.label("Get a Client ID"))
            .on_press(Message::OpenLink(DEVELOPER_DASHBOARD.into()))
            .into()
    }

    /// Edit a full configuration based on authoritative state, preserving untouched fields.
    /// Spicy Lyrics key entry; the saved key is never shown again.
    fn lyrics_key_field(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let placeholder: &str = if self.state.lyrics_key_set {
            "Replace the saved key"
        } else {
            "sl_sk_…"
        };
        let mut actions = row![self.control(self.label("Save key")).on_press_maybe(
            (!self.lyrics_key.trim().is_empty()).then_some(Message::SaveLyricsKey)
        )]
        .spacing(p.shape.spacing);
        if self.state.lyrics_key_set {
            actions = actions.push(
                self.control(self.label("Remove key"))
                    .on_press(Message::RemoveLyricsKey),
            );
        }
        row![
            self.field(
                "Spicy Lyrics API key",
                text_input(placeholder, &self.lyrics_key)
                    .secure(true)
                    .on_input(Message::LyricsKey)
                    .on_submit(Message::SaveLyricsKey),
            ),
            actions,
        ]
        .spacing(p.shape.spacing * 2.0)
        .align_y(Alignment::End)
        .into()
    }

    pub(super) fn settings_view(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let account: String = match &self.state.auth {
            AuthState::Ready { user_id } => format!("Signed in as {user_id}"),
            AuthState::Restoring => "Signing in…".into(),
            AuthState::Authorizing => "Waiting for authorization in your browser".into(),
            AuthState::Blocked { reason } => reason.clone(),
            AuthState::NeedsClientId => "Add a Client ID below to connect".into(),
            AuthState::SignedOut => "Not signed in".into(),
        };
        let account_actions: Element<'_> = if matches!(self.state.auth, AuthState::Ready { .. }) {
            column![
                self.playback_authorization(),
                self.control(self.icon_label(Icon::LogOut, "Sign out"))
                    .on_press(Message::Action(Action::Logout)),
            ]
            .spacing(p.shape.spacing * 1.5)
            .into()
        } else {
            row![
                self.primary("Authorize in browser", Message::Authorize),
                self.control(self.label("Cancel"))
                    .on_press(Message::Action(Action::CancelLogin)),
            ]
            .spacing(p.shape.spacing)
            .into()
        };
        let used: String = format!(
            "{:.1} MB used of {:.0} MB",
            self.state.cache_used_bytes as f64 / 1_000_000.0,
            self.state.cache_limit_bytes as f64 / 1_000_000.0
        );
        let page = column![
            text("Settings")
                .font(p.bold_font())
                .size(p.font_size.display),
            self.setting_section(
                "Appearance",
                "Themes change colors, shapes and type. Layout stays the same.",
                column![
                    self.theme_choices(),
                    self.zoom_control(),
                    self.title_bar_choice(),
                    self.field(
                        "Custom theme file",
                        text_input("/path/to/theme.toml", &self.theme_path)
                            .on_input(Message::ThemePath)
                            .on_submit(Message::Save),
                    ),
                ]
                .spacing(p.shape.spacing * 2.0)
                .into(),
            ),
            self.setting_section(
                "Account",
                &account,
                account_actions,
            ),
            self.setting_section(
                "Playback",
                "When an album, playlist or song ends, keep playing similar songs. A change applies the next time Scytify starts.",
                self.autoplay_choice(),
            ),
            self.setting_section(
                "Spotify developer app",
                &format!("Create an app in Spotify's developer dashboard, register exactly {} as its redirect URI, and paste its Client ID here. Use the public Client ID, never a client secret. Playback and Spotify Connect use a separate one-time browser consent above.", crate::backend::REDIRECT_URI),
                row![
                    self.field(
                        "Client ID",
                        text_input("Client ID", &self.client)
                            .on_input(Message::Client)
                            .on_submit(Message::Save),
                    ),
                    self.dashboard_link(),
                ]
                .spacing(p.shape.spacing * 2.0)
                .align_y(Alignment::End)
                .into(),
            ),
            self.setting_section(
                "Lyrics",
                if self.state.lyrics_key_set {
                    "Synced lyrics come from Spicy Lyrics. A key is saved in your system keyring."
                } else {
                    "Synced lyrics come from Spicy Lyrics. Create a free API key at developers.spicylyrics.org; it is kept in your system keyring."
                },
                self.lyrics_key_field(),
            ),
            self.setting_section(
                "Storage",
                &used,
                row![
                    self.field(
                        "Cache budget in bytes, shared by metadata and audio",
                        text_input("1000000000", &self.cache)
                            .on_input(Message::Cache)
                            .on_submit(Message::Save),
                    ),
                    self.control(self.label("Clear cache"))
                        .on_press(Message::Action(Action::ClearCache)),
                ]
                .spacing(p.shape.spacing * 2.0)
                .align_y(Alignment::End)
                .into(),
            ),
            row![self.primary("Save changes", Message::Save)],
            rule::horizontal(1).style(styles::divider(p)),
            self.setting_section(
                "Session",
                "Quit stops playback and saves the current queue.",
                self.control(self.icon_label(Icon::Power, "Quit Scytify"))
                    .on_press(Message::Action(Action::Quit))
                    .into(),
            ),
            self.setting_section(
                "Keyboard shortcuts",
                "Single keys work while no text field is focused.",
                self.shortcut_hints(),
            ),
        ]
        .spacing(p.shape.spacing * 4.0)
        .max_width(720);
        scrollable(container(page).padding(CONTENT_PADDING).width(Length::Fill))
            .id(iced::widget::Id::new(MAIN_SCROLL))
            .direction(thin_scrollbar())
            .style(styles::scrollbar(p))
            .height(Length::Fill)
            .into()
    }

    /// Shared account and device-picker consent control.
    pub(super) fn playback_authorization(&self) -> Element<'_> {
        let hint = match &self.state.connect.playback_auth {
            PlaybackAuth::Ready => {
                return self.subdued("Playback & Spotify Connect enabled").into();
            }
            PlaybackAuth::Authorizing => {
                return column![
                    self.subdued("Waiting for playback authorization in your browser…"),
                    self.control(self.label("Cancel playback authorization"))
                        .on_press(Message::Action(Action::CancelLogin)),
                ]
                .spacing(self.palette.shape.spacing)
                .into();
            }
            PlaybackAuth::NeedsAuthorization => {
                "Local audio and Connect need one extra Spotify consent. Remote playback controls remain available."
            }
            PlaybackAuth::Failed(error) => error.as_str(),
        };
        column![
            self.subdued(hint),
            self.control(self.label("Enable playback & Spotify Connect"))
                .on_press(Message::Action(Action::AuthorizePlayback)),
            self.subdued("Opens Spotify in your browser once"),
        ]
        .spacing(self.palette.shape.spacing)
        .into()
    }

    /// Heading, explanation and the controls of one settings group.
    fn setting_section<'a>(
        &self,
        title: &'a str,
        description: &str,
        body: Element<'a>,
    ) -> Element<'a> {
        column![
            self.heading(title).size(self.palette.font_size.body + 4.0),
            self.label(description.to_owned())
                .color(self.palette.colors.text_subdued),
            body,
        ]
        .spacing(self.palette.shape.spacing * 1.5)
        .into()
    }

    /// Key combinations and what they do, in two aligned columns.
    fn shortcut_hints(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let rows = shortcuts::HINTS.iter().map(|(keys, action)| {
            row![
                container(
                    container(text(*keys).font(p.bold_font()).size(p.font_size.small))
                        .padding([3.0, 8.0])
                        .style(styles::key_cap(p))
                )
                .width(KEYS_WIDTH),
                self.label(*action).color(p.colors.text_subdued),
            ]
            .spacing(p.shape.spacing * 1.5)
            .align_y(Alignment::Center)
            .into()
        });
        column(rows).spacing(p.shape.spacing).into()
    }

    /// Zoom out, current zoom, zoom in and reset; mirrors Ctrl+- / Ctrl+= / Ctrl+0.
    fn zoom_control(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let zoom: f32 = self.zoom;
        let step = |label: &'static str, message: Message| {
            self.control(self.label(label)).on_press(message)
        };
        column![
            self.subdued("Zoom"),
            row![
                step("-", Message::SetZoom(chrome::step_zoom(zoom, false))),
                container(self.strong(format!("{:.0}%", zoom * 100.0))).center_x(64),
                step("+", Message::SetZoom(chrome::step_zoom(zoom, true))),
                step("Reset", Message::SetZoom(1.0)),
            ]
            .spacing(p.shape.spacing)
            .align_y(Alignment::Center),
        ]
        .spacing(6)
        .into()
    }

    /// Autoplay on or off; saved immediately.
    fn autoplay_choice(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let choice = |label: &'static str, enabled: bool| {
            button(self.label(label))
                .padding([9.0, p.shape.padding])
                .style(styles::choice(p, self.state.settings.autoplay == enabled))
                .on_press(Message::SetAutoplay(enabled))
        };
        column![
            self.subdued("Autoplay"),
            row![choice("On", true), choice("Off", false)].spacing(p.shape.spacing),
        ]
        .spacing(6)
        .into()
    }

    /// The app's own title bar or the window system's; applies immediately.
    fn title_bar_choice(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let choice = |label: &'static str, system: bool| {
            button(self.label(label))
                .padding([9.0, p.shape.padding])
                .style(styles::choice(p, self.system_title_bar == system))
                .on_press(Message::SystemTitleBar(system))
        };
        column![
            self.subdued("Title bar"),
            row![
                choice("App title bar", false),
                choice("System title bar", true)
            ]
            .spacing(p.shape.spacing),
        ]
        .spacing(6)
        .into()
    }

    /// Labelled, themed text field.
    fn field<'a>(&self, label: &'a str, input: text_input::TextInput<'a, Message>) -> Element<'a> {
        let p: &Palette = &self.palette;
        column![
            self.subdued(label),
            input
                .font(p.font())
                .size(p.font_size.body)
                .style(styles::text_input(p))
                .padding([10.0, 12.0]),
        ]
        .spacing(6)
        .width(Length::Fill)
        .into()
    }

    /// Bundled themes as swatch cards; the active one is outlined in the accent.
    fn theme_choices(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let active: Option<&std::path::Path> = self.state.settings.theme_path.as_deref();
        let mut choices = row![].spacing(p.shape.spacing * 1.5);
        for choice in &self.themes {
            let c = choice.palette.colors;
            let swatches = row(
                [c.background, c.surface_raised, c.accent, c.text].map(|color| {
                    container(space())
                        .width(20)
                        .height(20)
                        .style(move |_: &iced::Theme| container::Style {
                            background: Some(color.into()),
                            border: iced::Border::default()
                                .rounded(10)
                                .color(p.colors.border)
                                .width(1),
                            ..container::Style::default()
                        })
                        .into()
                }),
            )
            .spacing(4);
            let selected: bool = choice.path.as_deref() == active;
            choices = choices.push(
                button(column![swatches, self.label(&choice.name)].spacing(10))
                    .padding(14)
                    .width(170)
                    .style(styles::choice(p, selected))
                    .on_press(Message::ChooseTheme(choice.path.clone())),
            );
        }
        choices.into()
    }
}
