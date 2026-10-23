//! Right panel with the playing track's lyrics on the cover's color: sung lines dim, the sung line fills
//! word by word, and the provider credit stays on screen below the lyrics.
use super::sidebar::thin_scrollbar;
use super::*;
use crate::frontend::lyrics::{self as sync, Phase, Status};
use iced::widget::column;
use iced::widget::text::Span;
use iced::widget::{mouse_area, rich_text, scrollable, sensor, span};

/// Opacity of lines still to come, and of lines already sung.
const UPCOMING_ALPHA: f32 = 0.35;
const SUNG_ALPHA: f32 = 0.6;
/// Space above and below the credit under the lyrics.
const CREDIT_PADDING: f32 = 10.0;
const SPICY_LYRICS_URL: &str = "https://spicylyrics.org";

impl Ui {
    pub(super) fn lyrics_view(&self) -> Element<'_> {
        let tint: Option<iced::Color> = self
            .state
            .player
            .current
            .as_ref()
            .and_then(|t| t.artwork_url())
            .and_then(|url| self.artwork.tint(url));
        let body: Element<'_> = match (&self.lyrics.status, self.lyrics.lyrics()) {
            (_, Some(lyrics)) => column![self.lyrics_scroll(lyrics), self.lyrics_credit(lyrics)].into(),
            _ if self.state.player.current.is_none() => self.lyrics_notice(
                "Nothing playing",
                "Play a song to see its lyrics here.".into(),
                None,
            ),
            _ if !self.state.lyrics_key_set => self.lyrics_notice(
                "Add a lyrics key",
                "Synced lyrics come from Spicy Lyrics. Create a free API key at developers.spicylyrics.org and add it in Settings.".into(),
                Some(("Open Settings", Message::Settings)),
            ),
            (Some(Status::Missing), _) => self.lyrics_notice(
                "No lyrics for this song",
                "Spicy Lyrics doesn't have lyrics for it yet.".into(),
                None,
            ),
            (Some(Status::Failed(e)), _) => self.lyrics_notice(
                "Couldn't load lyrics",
                e.message.clone(),
                Some(("Try again", Message::LyricsRetry)),
            ),
            _ => self.lyrics_notice("Loading lyrics…", String::new(), None),
        };
        let p: &Palette = &self.palette;
        let header = row![
            self.heading("Lyrics").size(p.font_size.body + 4.0),
            space().width(Length::Fill),
            self.icon_button(Icon::Close, 18.0, false, Message::Lyrics),
        ]
        .align_y(Alignment::Center)
        .padding(iced::Padding::from([p.shape.padding, sync::SIDE_PADDING]).bottom(0.0));
        let panel = container(column![header, body].height(Length::Fill))
            .width(Length::Fill)
            .height(Length::Fill)
            .clip(true)
            .style(styles::lyrics_backdrop(p, tint));
        container(styles::raised(p, p.shape.radius_large, panel))
            .width(self.panel_widths().1)
            .height(Length::Fill)
            .into()
    }

    /// Every line in a scroll area the follow animation moves; a click seeks to a line.
    fn lyrics_scroll<'a>(&'a self, lyrics: &'a Lyrics) -> Element<'a> {
        let height: f32 = self.lyrics.viewport.map_or(0.0, |v| v.height);
        let position: u64 = self.position_ms();
        let timed: bool = lyrics.sync != LyricsSync::Static;
        let lines = column(
            lyrics
                .lines
                .iter()
                .map(|line| self.lyrics_line(line, lyrics.sync, position, timed)),
        )
        .spacing(sync::LINE_GAP)
        .width(Length::Fill);
        let area = scrollable(
            container(lines).width(Length::Fill).padding(
                iced::Padding::from([0.0, sync::SIDE_PADDING])
                    .top(sync::top_padding(height))
                    .bottom(sync::bottom_padding(height)),
            ),
        )
        .id(Area::Lyrics.id())
        .on_scroll(|v| Message::Scrolled(Area::Lyrics, Metrics::of(&v)))
        .direction(thin_scrollbar())
        .style(styles::scrollbar(&self.palette))
        .width(Length::Fill)
        .height(Length::Fill);
        sensor(smooth_wheel(area, Area::Lyrics))
            .on_show(Message::LyricsResized)
            .on_resize(Message::LyricsResized)
            .into()
    }

    fn lyrics_line<'a>(
        &'a self,
        line: &'a LyricsLine,
        sync_kind: LyricsSync,
        position: u64,
        timed: bool,
    ) -> Element<'a> {
        let phase: Phase = if timed {
            sync::phase(line, position)
        } else {
            Phase::Singing
        };
        let align = if line.opposite {
            iced::alignment::Horizontal::Right
        } else {
            iced::alignment::Horizontal::Left
        };
        let words = |syllables: &'a [Syllable], size: f32| {
            let spans: Vec<Span<'a, (), iced::Font>> = sync::pieces(syllables)
                .zip(syllables)
                .map(|(piece, syllable)| {
                    span(piece).color(self.syllable_color(syllable, sync_kind, phase, position))
                })
                .collect();
            rich_text(spans)
                .font(self.palette.bold_font())
                .size(size)
                .line_height(sync::line_height())
                .wrapping(iced::widget::text::Wrapping::Word)
                .width(Length::Fill)
                .align_x(align)
        };
        let mut content = column![words(&line.lead, sync::LEAD_SIZE)]
            .spacing(sync::BACKGROUND_GAP)
            .width(Length::Fill);
        if !line.background.is_empty() {
            content = content.push(words(&line.background, sync::BACKGROUND_SIZE));
        }
        if !timed {
            return content.into();
        }
        mouse_area(content)
            .on_press(Message::Action(Action::Seek {
                position_ms: line.start_ms,
            }))
            .interaction(iced::mouse::Interaction::Pointer)
            .into()
    }

    /// Word-synced lines fill each syllable over the time it is sung; line-synced lines
    /// light up whole.
    fn syllable_color(
        &self,
        syllable: &Syllable,
        sync_kind: LyricsSync,
        phase: Phase,
        position: u64,
    ) -> iced::Color {
        let text: iced::Color = self.palette.colors.text;
        let alpha: f32 = match phase {
            Phase::Upcoming => UPCOMING_ALPHA,
            Phase::Sung => SUNG_ALPHA,
            Phase::Singing if sync_kind == LyricsSync::Syllable => {
                UPCOMING_ALPHA + (1.0 - UPCOMING_ALPHA) * sync::progress(syllable, position)
            }
            Phase::Singing => 1.0,
        };
        iced::Color { a: alpha, ..text }
    }

    /// "Lyrics from Spicy Lyrics" with the source, and the community contributors as links.
    /// The provider's terms require this next to the lyrics.
    fn lyrics_credit<'a>(&'a self, lyrics: &'a Lyrics) -> Element<'a> {
        let p: &Palette = &self.palette;
        let subdued: iced::Color = iced::Color {
            a: 0.75,
            ..p.colors.text
        };
        let link = |label: &'a str, url: &'a str| -> Span<'a, String, iced::Font> {
            span(label)
                .color(p.colors.text)
                .underline(true)
                .link(url.to_owned())
        };
        let plain = |label: &'a str| -> Span<'a, String, iced::Font> { span(label).color(subdued) };
        let credit: &LyricsCredit = &lyrics.credit;
        let mut spans: Vec<Span<'a, String, iced::Font>> = vec![
            plain("Lyrics from "),
            link("Spicy Lyrics", SPICY_LYRICS_URL),
        ];
        match credit.source {
            LyricsSource::Community => {
                if let Some(uploader) = &credit.uploader {
                    spans.push(plain(" · uploaded by "));
                    spans.push(link(&uploader.name, &uploader.url));
                }
                if let Some(maker) = &credit.maker {
                    spans.push(plain(if credit.uploader.is_some() {
                        ", made by "
                    } else {
                        " · made by "
                    }));
                    spans.push(link(&maker.name, &maker.url));
                }
            }
            LyricsSource::AppleMusic => spans.push(plain(" · source: Apple Music")),
            LyricsSource::Spotify => spans.push(plain(" · source: Spotify")),
            LyricsSource::Unknown => spans.push(plain(" · source unknown")),
        }
        container(
            rich_text(spans)
                .on_link_click(Message::OpenLink)
                .font(p.font())
                .size(p.font_size.small)
                .wrapping(iced::widget::text::Wrapping::Word),
        )
        .width(Length::Fill)
        .padding([CREDIT_PADDING, sync::SIDE_PADDING])
        .align_y(Alignment::Center)
        .into()
    }

    /// A centered message on the backdrop with an optional action.
    fn lyrics_notice<'a>(
        &'a self,
        title: &'a str,
        hint: String,
        action: Option<(&'a str, Message)>,
    ) -> Element<'a> {
        let p: &Palette = &self.palette;
        let mut content = column![]
            .spacing(p.shape.spacing * 1.5)
            .align_x(Alignment::Center)
            .max_width(360);
        if p.icons == Icons::Lucide {
            content = content.push(icons::icon(Icon::Lyrics, 40.0));
        }
        content = content.push(self.heading(title));
        if !hint.is_empty() {
            content = content.push(
                self.label(hint)
                    .align_x(iced::alignment::Horizontal::Center)
                    .wrapping(Wrapping::WordOrGlyph),
            );
        }
        if let Some((label, message)) = action {
            content = content.push(self.primary(label, message));
        }
        container(content)
            .center(Length::Fill)
            .padding(sync::SIDE_PADDING)
            .into()
    }
}
