//! Listening stats page: headline numbers, listening by hour of day, and the most played
//! songs and artists, recorded on this device and any devices linked over Tailscale.
use super::*;
use crate::frontend::artwork::Shape;
use crate::frontend::layout::{CONTENT_PADDING, SECTION_HEADING};
use iced::widget::{column, mouse_area, rule, text_input};

/// Width of one headline tile; tiles wrap onto more rows in narrow windows.
const TILE_WIDTH: f32 = 168.0;
/// Height of the tallest bar of the hour chart.
const CHART_HEIGHT: f32 = 140.0;
/// Main view width from which top songs and top artists sit side by side.
const SIDE_BY_SIDE: f32 = 880.0;
const RANK_WIDTH: f32 = 24.0;
const THUMB: f32 = 40.0;
/// Plays and minutes at the end of a song row.
const FIGURES_WIDTH: f32 = 128.0;
/// Minutes and plays next to an artist's name.
const ARTIST_FIGURES_WIDTH: f32 = 160.0;
const ROW_PADDING: f32 = 8.0;
const GAP: f32 = 12.0;

impl Ui {
    pub(super) fn stats_view<'a>(&'a self, stats: &'a ListeningStats) -> Element<'a> {
        let p: &Palette = &self.palette;
        let mut page = column![self.stats_header(stats.range)].spacing(p.shape.spacing * 2.0);
        if self.sync_open {
            page = page.push(container(self.sync_panel()).padding([0.0, CONTENT_PADDING]));
        }
        if stats.listened_ms == 0 {
            let hint: &'static str = if stats.range == StatsRange::AllTime {
                "Play some music and your stats will build up here."
            } else {
                "Nothing was played in this range. Try All time."
            };
            return page
                .push(self.centered_notice(Icon::Stats, "No listening yet".into(), hint))
                .into();
        }
        page = page
            .push(self.stats_tiles(stats))
            .push(self.hour_chart(&stats.by_hour));
        let wide: bool = self.grid_width() >= SIDE_BY_SIDE;
        let (songs_width, artists_width): (f32, f32) = if wide {
            let shared: f32 = self.grid_width() - GAP * 2.0;
            (shared * 0.6, shared * 0.4)
        } else {
            (self.grid_width(), self.grid_width())
        };
        let songs = self.top_songs(&stats.top_tracks, songs_width);
        let artists = self.top_artists(&stats.top_artists, artists_width);
        let rankings: Element<'a> = if wide {
            row![
                container(songs).width(Length::FillPortion(3)),
                container(artists).width(Length::FillPortion(2)),
            ]
            .spacing(GAP * 2.0)
            .into()
        } else {
            column![songs, artists]
                .spacing(p.shape.spacing * 2.0)
                .into()
        };
        page.push(container(rankings).padding([0.0, CONTENT_PADDING]))
            .into()
    }

    /// Page title, where the numbers come from, and the range choice.
    fn stats_header(&self, range: StatsRange) -> Element<'_> {
        let p: &Palette = &self.palette;
        let mut ranges = row![].spacing(8);
        for choice in StatsRange::ALL {
            let label = match choice {
                StatsRange::Week => "Last 7 days",
                StatsRange::Month => "Last 30 days",
                StatsRange::AllTime => "All time",
            };
            let pill = if choice == range {
                button(self.strong(label).color(p.colors.on_accent))
                    .style(styles::primary_button(p))
            } else {
                button(self.label(label)).style(styles::secondary_button(p))
            };
            ranges = ranges.push(
                pill.padding([7.0, 14.0])
                    .on_press(Message::StatsRange(choice)),
            );
        }
        let linked: usize = if self.state.sync.enabled {
            self.state.sync.peers.len()
        } else {
            0
        };
        let devices = button(self.icon_label(Icon::Devices, "Linked devices"))
            .padding([7.0, 14.0])
            .style(styles::choice(p, self.sync_open))
            .on_press(Message::SyncPanel);
        let source: String = match linked {
            0 => "Recorded on this device whenever Scytify sees music playing.".into(),
            1 => "Combined from this device and 1 linked device.".into(),
            n => format!("Combined from this device and {n} linked devices."),
        };
        column![
            text("Your stats")
                .font(p.bold_font())
                .size(p.font_size.display),
            self.subdued(source),
            container(
                row![ranges, space().width(Length::Fill), devices].align_y(Alignment::Center)
            )
            .padding(iced::Padding::ZERO.top(p.shape.spacing)),
        ]
        .spacing(6)
        .padding(iced::Padding::new(CONTENT_PADDING).bottom(0.0))
        .into()
    }

    /// Turning sync on, this device's link code, linking another device, and the linked
    /// devices with how their last exchange went.
    fn sync_panel(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let sync: &SyncState = &self.state.sync;
        let sync_action = |action: SyncAction| Message::Action(Action::Sync(action));
        let mut panel = column![self.heading("Linked devices")].spacing(p.shape.spacing * 1.5);
        if let Some(problem) = &sync.problem {
            panel = panel.push(self.label(problem.as_str()).color(p.colors.error));
        }
        if !sync.enabled {
            panel = panel.push(self.subdued(
                "Combine stats from your other computers. Each needs Scytify and Tailscale, signed in to the same tailnet.",
            ));
            panel = panel.push(row![
                self.primary("Turn on sync", sync_action(SyncAction::Enable(true)))
            ]);
            return container(panel)
                .padding(p.shape.padding)
                .style(styles::tile(p))
                .into();
        }
        let code: Element<'_> = match &sync.code {
            Some(code) => row![
                container(self.strong(code.as_str()).wrapping(Wrapping::Glyph)).width(Length::Fill),
                self.control(self.label("Copy"))
                    .on_press(Message::CopyLink(code.clone())),
            ]
            .spacing(p.shape.spacing)
            .align_y(Alignment::Center)
            .into(),
            None => self.subdued("Waiting for Tailscale…").into(),
        };
        panel = panel.push(
            column![
                self.subdued("To link another device, enter this device's code there"),
                code
            ]
            .spacing(6),
        );
        panel = panel.push(
            row![
                text_input("Paste a code from another device", &self.sync_code)
                    .on_input(Message::SyncCode)
                    .on_submit(Message::SyncJoin)
                    .font(p.font())
                    .size(p.font_size.body)
                    .style(styles::text_input(p))
                    .padding([10.0, 12.0]),
                self.control(self.label("Link")).on_press_maybe(
                    (!self.sync_code.trim().is_empty()).then_some(Message::SyncJoin)
                ),
            ]
            .spacing(p.shape.spacing)
            .align_y(Alignment::Center),
        );
        if sync.peers.is_empty() {
            panel = panel.push(self.subdued("No devices linked yet."));
        }
        let now: i64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        for peer in &sync.peers {
            let status: String = match (&peer.error, peer.last_synced) {
                (Some(error), _) => format!("Not reachable: {error}"),
                (None, Some(at)) => format!("Synced {}", ago(now - at)),
                (None, None) => "Not synced yet".into(),
            };
            panel = panel.push(
                row![
                    column![self.strong(peer.address.as_str()), self.subdued(status)]
                        .spacing(2)
                        .width(Length::Fill),
                    self.control(self.label("Unlink"))
                        .on_press(sync_action(SyncAction::Forget(peer.device_id.clone()))),
                ]
                .spacing(p.shape.spacing)
                .align_y(Alignment::Center),
            );
        }
        panel = panel.push(
            row![
                self.control(self.label("Sync now"))
                    .on_press(sync_action(SyncAction::Now)),
                self.control(self.label("Turn off sync"))
                    .on_press(sync_action(SyncAction::Enable(false))),
            ]
            .spacing(p.shape.spacing),
        );
        container(panel)
            .padding(p.shape.padding)
            .style(styles::tile(p))
            .into()
    }

    /// Headline numbers, each on its own tile.
    fn stats_tiles(&self, stats: &ListeningStats) -> Element<'_> {
        let minutes: u64 = stats.listened_ms / 60_000;
        let hours: String = format!("{:.1} hours", stats.listened_ms as f64 / 3_600_000.0);
        let streak: String = if stats.longest_streak_days > stats.streak_days {
            format!("Best: {} days", stats.longest_streak_days)
        } else if stats.streak_days > 0 {
            "Your best yet".into()
        } else {
            "Play today to start one".into()
        };
        let tiles = [
            self.tile("Minutes listened", grouped(minutes), hours),
            self.tile(
                "Plays",
                grouped(stats.plays),
                format!("{} min per play", average_minutes(stats)),
            ),
            self.tile(
                "Songs",
                grouped(stats.songs),
                "Different songs played".into(),
            ),
            self.tile(
                "Artists",
                grouped(stats.artists),
                "Different artists heard".into(),
            ),
            self.tile("Day streak", grouped(stats.streak_days), streak),
        ];
        container(row(tiles).spacing(GAP).wrap().vertical_spacing(GAP))
            .padding([0.0, CONTENT_PADDING])
            .into()
    }

    fn tile<'a>(&self, label: &'a str, value: String, detail: String) -> Element<'a> {
        let p: &Palette = &self.palette;
        container(
            column![
                self.subdued(label),
                text(value)
                    .font(p.bold_font())
                    .size(p.font_size.display)
                    .wrapping(Wrapping::None),
                self.subdued(detail).wrapping(Wrapping::None),
            ]
            .spacing(4),
        )
        .padding(p.shape.padding)
        .width(TILE_WIDTH)
        .clip(true)
        .style(styles::tile(p))
        .into()
    }

    /// Listening time by hour of day, the busiest hour in the accent color.
    fn hour_chart(&self, by_hour: &[u64; 24]) -> Element<'_> {
        let p: &Palette = &self.palette;
        let most: u64 = by_hour.iter().copied().max().unwrap_or(0);
        let peak: usize = by_hour.iter().position(|&ms| ms == most).unwrap_or(0);
        let mut bars = row![].spacing(2).align_y(Alignment::End);
        let mut ticks = row![].spacing(2);
        for (hour, &ms) in by_hour.iter().enumerate() {
            let height: f32 = if most == 0 {
                0.0
            } else {
                CHART_HEIGHT * ms as f32 / most as f32
            };
            // The hover target is the full column, not just the bar.
            let column = container(
                container(space())
                    .width(Length::Fill)
                    .height(height)
                    .style(styles::bar(p, hour == peak)),
            )
            .width(Length::Fill)
            .height(CHART_HEIGHT)
            .align_y(Alignment::End);
            let tip = format!("{} · {} min", hour_name(hour), ms / 60_000);
            bars = bars.push(iced::widget::tooltip(
                column,
                container(self.subdued(tip))
                    .padding([6.0, 10.0])
                    .style(styles::popover(p)),
                iced::widget::tooltip::Position::Top,
            ));
            let tick: Element<'_> = if hour % 6 == 0 {
                self.subdued(hour_name(hour))
                    .wrapping(Wrapping::None)
                    .into()
            } else {
                space().into()
            };
            ticks = ticks.push(container(tick).width(Length::Fill));
        }
        column![
            container(self.heading("When you listen"))
                .height(SECTION_HEADING)
                .align_y(Alignment::Center),
            self.label(format!("You listen most around {}.", hour_name(peak)))
                .color(p.colors.text_subdued),
            space().height(p.shape.spacing * 1.5),
            bars,
            rule::horizontal(1).style(styles::divider(p)),
            space().height(4),
            ticks,
        ]
        .padding([0.0, CONTENT_PADDING])
        .into()
    }

    /// Most played songs; a click plays one, a right click opens its actions.
    fn top_songs<'a>(&'a self, tracks: &'a [RankedTrack], width: f32) -> Element<'a> {
        let p: &Palette = &self.palette;
        let title_width: f32 =
            (width - 2.0 * ROW_PADDING - RANK_WIDTH - THUMB - FIGURES_WIDTH - 3.0 * GAP).max(0.0);
        let mut rows = column![].spacing(2);
        for (index, ranked) in tracks.iter().enumerate() {
            let track: &Track = &ranked.track;
            let current: bool = self
                .state
                .player
                .current
                .as_ref()
                .is_some_and(|t| t.id == track.id);
            let figures = column![
                self.label(plural(ranked.plays, "play")),
                self.subdued(format!("{} min", grouped(ranked.listened_ms / 60_000))),
            ]
            .align_x(Alignment::End)
            .width(FIGURES_WIDTH);
            let content = row![
                self.rank(index),
                self.cover(track.artwork_url(), THUMB, Shape::Cover),
                container(self.track_title(track, current, title_width)).width(Length::Fill),
                figures,
            ]
            .spacing(GAP)
            .align_y(Alignment::Center);
            let mut item = button(content)
                .padding([6.0, ROW_PADDING])
                .width(Length::Fill)
                .style(styles::list_row(p, false));
            if track.playable {
                item = item.on_press(Message::PlayTrack(
                    PlaySource::Track(track.id.clone()),
                    Box::new(track.clone()),
                ));
            }
            rows = rows
                .push(mouse_area(item).on_right_press(Message::TrackMenu(Box::new(track.clone()))));
        }
        column![self.stats_section("Top songs"), rows].into()
    }

    /// Artists by listening time, each with a meter relative to the first.
    fn top_artists<'a>(&'a self, artists: &'a [RankedArtist], width: f32) -> Element<'a> {
        let p: &Palette = &self.palette;
        let name_width: f32 =
            (width - 2.0 * ROW_PADDING - RANK_WIDTH - ARTIST_FIGURES_WIDTH - 2.0 * GAP).max(0.0);
        let most: u64 = artists.first().map_or(0, |a| a.listened_ms).max(1);
        let mut rows = column![].spacing(p.shape.spacing * 1.5);
        for (index, ranked) in artists.iter().enumerate() {
            // Fill portions out of 1000, keeping a sliver for the smallest share.
            let filled: u16 = ((ranked.listened_ms * 1000 / most) as u16).clamp(8, 1000);
            let mut meter = row![
                container(space())
                    .height(6)
                    .width(Length::FillPortion(filled))
                    .style(styles::meter(p, true)),
            ];
            if filled < 1000 {
                meter = meter.push(space().width(Length::FillPortion(1000 - filled)));
            }
            let name = button(self.strong(self.fit_bold(
                &ranked.artist.name,
                name_width,
                p.font_size.body,
            )))
            .padding(0)
            .style(styles::link(p))
            .on_press(Message::Navigate(Route::artist(&ranked.artist)));
            let details = row![
                container(name).width(Length::Fill).clip(true),
                self.subdued(format!(
                    "{} min · {}",
                    grouped(ranked.listened_ms / 60_000),
                    plural(ranked.plays, "play")
                ))
                .wrapping(Wrapping::None)
                .width(ARTIST_FIGURES_WIDTH)
                .align_x(iced::alignment::Horizontal::Right),
            ]
            .spacing(GAP)
            .align_y(Alignment::Center);
            rows = rows.push(
                row![
                    self.rank(index),
                    column![
                        details,
                        container(meter)
                            .width(Length::Fill)
                            .style(styles::meter(p, false)),
                    ]
                    .spacing(8),
                ]
                .spacing(GAP)
                .align_y(Alignment::Center),
            );
        }
        column![
            self.stats_section("Top artists"),
            container(rows).padding([0.0, ROW_PADDING]),
        ]
        .into()
    }

    fn rank(&self, index: usize) -> Element<'_> {
        self.strong((index + 1).to_string())
            .color(self.palette.colors.text_subdued)
            .width(RANK_WIDTH)
            .align_x(iced::alignment::Horizontal::Center)
            .into()
    }

    fn stats_section<'a>(&self, title: &'a str) -> Element<'a> {
        container(self.heading(title))
            .height(SECTION_HEADING)
            .align_y(Alignment::Center)
            .into()
    }
}

/// "12 AM", "9 PM" for an hour of the day.
fn hour_name(hour: usize) -> String {
    let suffix: &str = if hour < 12 { "AM" } else { "PM" };
    let twelve: usize = match hour % 12 {
        0 => 12,
        h => h,
    };
    format!("{twelve} {suffix}")
}

/// Thousands separated with commas: 12,345.
fn grouped(value: u64) -> String {
    let digits: String = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn plural(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{} {noun}s", grouped(count))
    }
}

/// Average listening per play, to one decimal.
fn average_minutes(stats: &ListeningStats) -> String {
    match stats.plays {
        0 => "0".into(),
        plays => format!("{:.1}", stats.listened_ms as f64 / plays as f64 / 60_000.0),
    }
}

/// How long ago, roughly, `seconds` were.
fn ago(seconds: i64) -> String {
    match seconds {
        ..60 => "just now".into(),
        60..3_600 => format!("{} min ago", seconds / 60),
        3_600..86_400 => format!("{} h ago", seconds / 3_600),
        _ => format!("{} days ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_naturally() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(hour_name(0), "12 AM");
        assert_eq!(hour_name(12), "12 PM");
        assert_eq!(hour_name(21), "9 PM");
        assert_eq!(plural(1, "play"), "1 play");
        assert_eq!(plural(2_000, "play"), "2,000 plays");
    }
}
