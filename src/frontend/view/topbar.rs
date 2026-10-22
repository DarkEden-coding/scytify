//! App icon, history arrows, search, the queue/settings toggles and the window buttons above
//! the main view. With the custom title bar, empty space in the bar drags the window.
use super::*;
use iced::widget::{image, mouse_area, rule, text_input};

/// Height of the top bar; the scroll area below it starts here.
pub(super) const TOP_BAR_HEIGHT: f32 = 64.0;
/// Edge of the app icon at the left of the custom title bar.
const APP_ICON: f32 = 24.0;

impl Ui {
    pub(super) fn top_bar(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let mut back = self.history_button(Icon::ChevronLeft);
        if self.history.can_go_back() {
            back = back.on_press(Message::Back);
        }
        let mut forward = self.history_button(Icon::ChevronRight);
        if self.history.can_go_forward() {
            forward = forward.on_press(Message::Forward);
        }
        let mut search = text_input("What do you want to play?", &self.search)
            .id(iced::widget::Id::new(SEARCH_INPUT))
            .font(p.font())
            .size(p.font_size.body)
            .style(styles::pill_input(p))
            .on_input(Message::Search)
            .on_submit(Message::SearchNow)
            .padding([10.0, 16.0])
            .width(Length::Fill);
        if p.icons == Icons::Lucide {
            search = search.icon(text_input::Icon {
                font: icons::FONT,
                code_point: Icon::Search.codepoint(),
                size: Some(18.into()),
                spacing: 10.0,
                side: text_input::Side::Left,
            });
        }
        let custom: bool = self.custom_chrome();
        let mut bar = row![].spacing(p.shape.spacing).align_y(Alignment::Center);
        if custom {
            bar = bar
                .push(
                    image(self.app_icon.clone())
                        .width(APP_ICON)
                        .height(APP_ICON),
                )
                .push(space().width(p.shape.spacing));
        }
        bar = bar
            .push(back)
            .push(forward)
            .push(space().width(p.shape.spacing))
            // Shrinks before anything else so the window buttons never clip on narrow windows.
            .push(container(search).width(Length::Fill).max_width(380.0))
            .push(space().width(Length::Fill))
            .push(self.icon_button(Icon::ListMusic, 20.0, self.queue, Message::Queue))
            .push(self.icon_button(Icon::Settings, 20.0, self.settings, Message::Settings));
        let mut padding = iced::Padding::from([0.0, layout::CONTENT_PADDING - 8.0]);
        if custom {
            bar = bar
                .push(container(rule::vertical(1).style(styles::divider(p))).height(20))
                .push(self.window_buttons());
            padding.right = p.shape.spacing;
        }
        let bar = container(bar)
            .padding(padding)
            .height(TOP_BAR_HEIGHT)
            .center_y(TOP_BAR_HEIGHT);
        if custom {
            mouse_area(bar)
                .on_press(Message::DragWindow)
                .on_double_click(Message::ToggleMaximize)
                .into()
        } else {
            bar.into()
        }
    }

    /// Minimize, maximize or restore, and close.
    fn window_buttons(&self) -> Element<'_> {
        let p: &Palette = &self.palette;
        let maximize: Icon = if self.maximized {
            Icon::Restore
        } else {
            Icon::Maximize
        };
        let window_button = |which: Icon, close: bool, message: Message| {
            button(glyph(p, which, 16.0))
                .padding([8.0, 12.0])
                .style(styles::window_button(p, close))
                .on_press(message)
        };
        row![
            window_button(Icon::Minimize, false, Message::Minimize),
            window_button(maximize, false, Message::ToggleMaximize),
            window_button(Icon::Close, true, Message::CloseWindow),
        ]
        .spacing(2)
        .into()
    }

    /// Round back/forward button; disabled without history in that direction.
    fn history_button<'a>(&self, which: Icon) -> iced::widget::Button<'a, Message> {
        button(glyph(&self.palette, which, 20.0))
            .padding(6)
            .style(styles::history_button(&self.palette))
    }
}
