//! Reusable widget styles derived from the active theme's design tokens.
//!
//! Each helper copies the palette into the returned closure, so it can be passed
//! straight to `.style(...)`. Panels follow `shadow.style` through [`raised`]: flat
//! themes draw no shadow, soft themes one drop shadow and neumorphic themes a dark
//! shadow paired with a light highlight. No style sets Iced's own `Shadow`: the
//! tiny-skia renderer draws it without the clip mask, so it leaked out of scroll areas
//! and accumulated over panels on every partial redraw.
use super::theme::{Palette, ShadowStyle, mix};
use super::tint;
use iced::widget::{self, button, container, rule, scrollable};
use iced::{Background, Border, Color, Element, Shadow, Theme, Vector};

/// Radius large enough to turn any button into a pill or circle.
const PILL: f32 = 999.0;

/// Accent-filled pill for the main action, such as the big play button.
pub fn primary_button(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let background: Color = match status {
            button::Status::Active => c.accent,
            button::Status::Hovered => mix(c.accent, Color::WHITE, 0.12),
            button::Status::Pressed => mix(c.accent, Color::BLACK, 0.12),
            button::Status::Disabled => c.accent.scale_alpha(0.4),
        };
        button::Style {
            background: Some(background.into()),
            text_color: c.on_accent,
            border: Border::default().rounded(PILL),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Raised-surface button for ordinary actions such as "Clear cache".
pub fn secondary_button(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let (background, text_color): (Color, Color) = match status {
            button::Status::Active => (c.surface_raised, c.text),
            button::Status::Hovered => (c.surface_hover, c.text),
            button::Status::Pressed => (c.surface_raised, c.text_subdued),
            button::Status::Disabled => (c.surface_raised, c.text_subdued.scale_alpha(0.6)),
        };
        button::Style {
            background: Some(background.into()),
            text_color,
            border: outline(&p, p.shape.radius_small),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Transparent icon button; brightens on hover and uses the accent while `active`
/// (for example shuffle or repeat turned on).
pub fn icon_button(
    p: &Palette,
    active: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let text_color: Color = match status {
            button::Status::Disabled => c.text_subdued.scale_alpha(0.4),
            _ if active => c.accent,
            button::Status::Hovered => c.text,
            button::Status::Active | button::Status::Pressed => c.text_subdued,
        };
        button::Style {
            background: None,
            text_color,
            border: Border::default().rounded(PILL),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Minimize, maximize and close buttons of the custom title bar: subdued until hovered;
/// `close` turns the danger color with its own icon color instead of the hover surface.
pub fn window_button(
    p: &Palette,
    close: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let hovered: bool = matches!(status, button::Status::Hovered | button::Status::Pressed);
        let (background, text_color): (Option<Background>, Color) = match (hovered, close) {
            (true, true) => (Some(c.danger_hover.into()), c.on_danger),
            (true, false) => (Some(c.surface_hover.into()), c.text),
            (false, _) => (None, c.text_subdued),
        };
        button::Style {
            background,
            text_color,
            border: Border::default().rounded(p.shape.radius_small),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Transparent text button for navigation links; `active` marks the current page.
pub fn ghost_button(
    p: &Palette,
    active: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let text_color: Color = match status {
            button::Status::Disabled => c.text_subdued.scale_alpha(0.4),
            _ if active => c.text,
            button::Status::Hovered => c.text,
            button::Status::Active | button::Status::Pressed => c.text_subdued,
        };
        button::Style {
            background: None,
            text_color,
            border: Border::default().rounded(p.shape.radius_small),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Flat list row (track, playlist, queue entry). `selected` highlights the row
/// and draws its text in the accent color.
pub fn list_row(
    p: &Palette,
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let background: Option<Color> = match status {
            _ if selected => Some(c.surface_raised),
            button::Status::Hovered => Some(c.surface_hover),
            button::Status::Pressed => Some(c.surface_raised),
            button::Status::Active | button::Status::Disabled => None,
        };
        let text_color: Color = match status {
            button::Status::Disabled => c.text_subdued,
            _ if selected => c.accent,
            _ => c.text,
        };
        button::Style {
            background: background.map(Background::from),
            text_color,
            border: Border::default().rounded(p.shape.radius_small),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Clickable album, artist or playlist card in a grid. Flat themes show the card only
/// on hover, as Spotify does; raised themes keep a raised surface with a faint rim.
pub fn card(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let flat: bool = p.shadow.style == ShadowStyle::Flat;
        let background: Option<Color> = match status {
            button::Status::Hovered | button::Status::Pressed => Some(c.surface_hover),
            button::Status::Active | button::Status::Disabled if flat => None,
            button::Status::Active | button::Status::Disabled => Some(c.surface_raised),
        };
        let border: Border = if flat {
            outline(&p, p.shape.radius_large)
        } else {
            Border {
                color: mix(c.surface_raised, c.text, 0.07),
                width: 1.0,
                radius: p.shape.radius_large.into(),
            }
        };
        button::Style {
            background: background.map(Background::from),
            text_color: c.text,
            border,
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// The search overview's top result: a card that keeps its surface on flat themes too.
pub fn top_result(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let card = card(p);
    let surface: Color = p.colors.surface_raised;
    move |theme, status| {
        let mut style: button::Style = card(theme, status);
        style.background.get_or_insert(surface.into());
        style
    }
}

/// Kind label ("Artist", "Album", "Song") on the top result card.
pub fn pill(p: &Palette) -> impl Fn(&Theme) -> container::Style + 'static {
    let c = p.colors;
    move |_| container::Style {
        background: Some(c.background.into()),
        text_color: Some(c.text),
        border: Border::default().rounded(PILL),
        ..container::Style::default()
    }
}

/// Dark veil behind the play glyph over a hovered song thumbnail.
pub fn scrim(p: &Palette) -> impl Fn(&Theme) -> container::Style + 'static {
    let radius: f32 = p.shape.radius_small;
    move |_| container::Style {
        background: Some(Color::BLACK.scale_alpha(0.55).into()),
        border: Border::default().rounded(radius),
        ..container::Style::default()
    }
}

/// Round button filled with the text color, for the player's play/pause.
pub fn inverse_button(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let background: Color = match status {
            button::Status::Hovered => mix(c.text, c.accent, 0.15),
            button::Status::Pressed => mix(c.text, c.background, 0.2),
            button::Status::Active => c.text,
            button::Status::Disabled => c.text.scale_alpha(0.4),
        };
        button::Style {
            background: Some(background.into()),
            text_color: c.background,
            border: Border::default().rounded(PILL),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Track row background: hover and selection use the surface tokens.
pub fn track_row(
    p: &Palette,
    hovered: bool,
    selected: bool,
) -> impl Fn(&Theme) -> container::Style + 'static {
    let p: Palette = *p;
    move |_| {
        let c = p.colors;
        let background: Option<Color> = if selected {
            Some(c.surface_raised)
        } else if hovered {
            Some(c.surface_hover)
        } else {
            None
        };
        container::Style {
            background: background.map(Background::from),
            text_color: Some(c.text),
            border: Border::default().rounded(p.shape.radius_small),
            ..container::Style::default()
        }
    }
}

/// Square or round placeholder shown while artwork is missing.
pub fn placeholder(p: &Palette, radius: f32) -> impl Fn(&Theme) -> container::Style + 'static {
    let c = p.colors;
    move |_| container::Style {
        background: Some(c.surface_raised.into()),
        border: Border::default().rounded(radius),
        ..container::Style::default()
    }
}

/// Liked Songs cover: a diagonal accent gradient behind the heart, standing in for art.
pub fn liked_cover(p: &Palette, radius: f32) -> impl Fn(&Theme) -> container::Style + 'static {
    let c = p.colors;
    move |_| {
        let gradient =
            iced::gradient::Linear::new(iced::Radians(std::f32::consts::FRAC_PI_4 * 3.0))
                .add_stop(0.0, mix(c.accent, Color::WHITE, 0.25))
                .add_stop(1.0, mix(c.accent, c.on_accent, 0.45));
        container::Style {
            background: Some(Background::Gradient(gradient.into())),
            text_color: Some(c.on_accent),
            border: Border::default().rounded(radius),
            ..container::Style::default()
        }
    }
}

/// Page header backdrop: the cover tint (or a faint accent without art) fading into the
/// panel surface. `split` is the fraction where the header ends and the play actions begin.
pub fn hero(
    p: &Palette,
    tint: Option<Color>,
    split: f32,
) -> impl Fn(&Theme) -> container::Style + 'static {
    let p: Palette = *p;
    move |_| {
        let c = p.colors;
        let top: Color = match tint {
            Some(color) => tint::readable(color, c.text),
            None => mix(c.surface_hover, c.accent, 0.12),
        };
        // Iced overwrites rather than inserts out-of-order stops, so add them in order.
        let mut gradient =
            iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI)).add_stop(0.0, top);
        if split < 1.0 {
            gradient = gradient.add_stop(split, mix(top, c.surface, 0.6));
        }
        let gradient = gradient.add_stop(1.0, c.surface);
        container::Style {
            background: Some(Background::Gradient(gradient.into())),
            text_color: Some(c.text),
            border: Border::default().rounded(p.shape.radius_large),
            ..container::Style::default()
        }
    }
}

/// Key combination label in the shortcut list.
pub fn key_cap(p: &Palette) -> impl Fn(&Theme) -> container::Style + 'static {
    let p: Palette = *p;
    move |_| container::Style {
        background: Some(p.colors.surface_raised.into()),
        text_color: Some(p.colors.text),
        border: Border {
            color: p.colors.border,
            width: 1.0,
            radius: 4.0.into(),
        },
        ..container::Style::default()
    }
}

/// Transparent-to-surface fade over the bottom edge of a side list, so a row cut off by
/// the panel reads as "more below" instead of a clipping glitch.
pub fn fade(p: &Palette) -> impl Fn(&Theme) -> container::Style + 'static {
    let surface: Color = p.colors.surface;
    move |_| {
        let gradient = iced::gradient::Linear::new(iced::Radians(std::f32::consts::PI))
            .add_stop(0.0, surface.scale_alpha(0.0))
            .add_stop(1.0, surface);
        container::Style {
            background: Some(Background::Gradient(gradient.into())),
            ..container::Style::default()
        }
    }
}

/// Small accent dot under an active transport toggle.
pub fn dot(p: &Palette) -> impl Fn(&Theme) -> container::Style + 'static {
    let c = p.colors;
    move |_| container::Style {
        background: Some(c.accent.into()),
        border: Border::default().rounded(PILL),
        ..container::Style::default()
    }
}

/// Dismissible notice floating above the player bar.
pub fn toast(p: &Palette) -> impl Fn(&Theme) -> container::Style + 'static {
    let p: Palette = *p;
    move |_| {
        let c = p.colors;
        container::Style {
            background: Some(c.surface_raised.into()),
            text_color: Some(c.text),
            // A neutral rim sets it apart in flat themes; the icon carries the error color.
            border: Border {
                color: c.border,
                width: 1.0,
                radius: p.shape.radius_large.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Floating surface above the layout, such as the device picker and tooltips. It always
/// has a rim, because flat themes have no border width and no shadow to set it apart.
pub fn popover(p: &Palette) -> impl Fn(&Theme) -> container::Style + 'static {
    let p: Palette = *p;
    move |_| {
        let c = p.colors;
        container::Style {
            background: Some(c.surface_raised.into()),
            text_color: Some(c.text),
            border: Border {
                color: c.border,
                width: p.shape.border_width.max(1.0),
                radius: p.shape.radius_large.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Full-width accent strip below the player ("Playing on ...") that opens the picker.
pub fn banner(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let background: Color = match status {
            button::Status::Hovered => mix(c.accent, Color::WHITE, 0.12),
            button::Status::Pressed => mix(c.accent, Color::BLACK, 0.12),
            button::Status::Active | button::Status::Disabled => c.accent,
        };
        button::Style {
            background: Some(background.into()),
            text_color: c.on_accent,
            border: Border::default(),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Volume rail of a device that cannot change its volume: the slider's rail, inert.
pub fn inert_rail(p: &Palette) -> impl Fn(&Theme) -> widget::progress_bar::Style + 'static {
    let c = p.colors;
    move |_| widget::progress_bar::Style {
        background: mix(c.surface, c.text, 0.12).into(),
        bar: mix(c.surface, c.text, 0.3).into(),
        border: Border::default().rounded(2.0),
    }
}

/// Selectable option such as a theme choice; the selected one gets an accent outline.
pub fn choice(
    p: &Palette,
    selected: bool,
) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let background: Color = match status {
            button::Status::Hovered | button::Status::Pressed => c.surface_hover,
            _ => c.surface_raised,
        };
        button::Style {
            background: Some(background.into()),
            text_color: c.text,
            border: Border {
                color: if selected { c.accent } else { c.border },
                width: if selected { 2.0 } else { 1.0 },
                radius: p.shape.radius_small.into(),
            },
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Window background behind all panels.
pub fn root(p: &Palette) -> impl Fn(&Theme) -> container::Style + 'static {
    let c = p.colors;
    move |_| container::Style {
        background: Some(c.background.into()),
        text_color: Some(c.text),
        ..container::Style::default()
    }
}

/// Surface panel such as the sidebar, main view or player bar.
pub fn panel(p: &Palette) -> impl Fn(&Theme) -> container::Style + 'static {
    let p: Palette = *p;
    move |_| container::Style {
        background: Some(p.colors.surface.into()),
        text_color: Some(p.colors.text),
        border: outline(&p, p.shape.radius_large),
        shadow: Shadow::default(),
        snap: true,
    }
}

/// Lyrics backdrop: the cover's color made readable, like the page header tint.
pub fn lyrics_backdrop(
    p: &Palette,
    tint: Option<Color>,
) -> impl Fn(&Theme) -> container::Style + 'static {
    let p: Palette = *p;
    move |_| {
        let c = p.colors;
        let color: Color = match tint {
            Some(color) => tint::readable(color, c.text),
            None => mix(c.surface_hover, c.accent, 0.12),
        };
        container::Style {
            background: Some(color.into()),
            text_color: Some(c.text),
            border: outline(&p, p.shape.radius_large),
            ..container::Style::default()
        }
    }
}

/// Panel margin reserved for an emulated shadow; zero for flat themes.
pub fn shadow_margin(p: &Palette) -> f32 {
    match p.shadow.style {
        ShadowStyle::Flat => 0.0,
        ShadowStyle::Soft | ShadowStyle::Neumorphic => p.shadow.offset.max(0.0).round(),
    }
}

/// Raise a fixed panel with the theme's shadow.
///
/// Iced's tiny-skia renderer draws `Shadow` without the clip mask, so every partial
/// redraw (a hovered row, a progress tick) painted the shadow again over the panel and
/// darkened it a little more. This emulates the shadow with plain translucent quads in a
/// `shadow_margin` band around the panel instead: three offset layers per shadow give a
/// soft falloff, dark below (soft) or below-right (neumorphic), plus a light layer
/// above-left for neumorphic themes. Fills respect clipping, so redraws are exact.
pub fn raised<'a, Message: 'a>(
    p: &Palette,
    radius: f32,
    content: impl Into<Element<'a, Message>>,
) -> Element<'a, Message> {
    let margin: f32 = shadow_margin(p);
    if margin == 0.0 {
        return content.into();
    }
    let strength: f32 = p.shadow.strength;
    let dark: Color = Color::BLACK.scale_alpha(strength / 3.0);
    let light: Color = mix(p.colors.surface, Color::WHITE, 0.8).scale_alpha(strength * 0.25 / 3.0);
    let neumorphic: bool = p.shadow.style == ShadowStyle::Neumorphic;
    let mut layers = widget::Stack::new()
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .push(container(content).padding(margin));
    for step in 1..=3 {
        let d: f32 = margin * step as f32 / 3.0;
        let dark_offset: Vector = if neumorphic {
            Vector::new(d, d)
        } else {
            Vector::new(0.0, d)
        };
        layers = layers.push_under(shadow_layer(margin, dark_offset, radius, dark));
        if neumorphic {
            layers = layers.push_under(shadow_layer(margin, Vector::new(-d, -d), radius, light));
        }
    }
    layers.into()
}

/// One translucent rounded rectangle: the panel's rectangle moved by `offset`.
fn shadow_layer<'a, Message: 'a>(
    margin: f32,
    offset: Vector,
    radius: f32,
    color: Color,
) -> Element<'a, Message> {
    let shape = container(widget::space())
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .style(move |_| container::Style {
            background: Some(color.into()),
            border: Border::default().rounded(radius),
            ..container::Style::default()
        });
    container(shape)
        .padding(iced::Padding {
            top: margin + offset.y,
            right: margin - offset.x,
            bottom: margin - offset.y,
            left: margin + offset.x,
        })
        .into()
}

/// Search and settings fields.
pub fn text_input(
    p: &Palette,
) -> impl Fn(&Theme, widget::text_input::Status) -> widget::text_input::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let base: Border = outline(&p, p.shape.radius_small);
        let (background, border, value): (Color, Border, Color) = match status {
            widget::text_input::Status::Active => (c.surface_raised, base, c.text),
            widget::text_input::Status::Hovered => (c.surface_hover, base, c.text),
            widget::text_input::Status::Focused { .. } => {
                (c.surface_hover, base.color(c.focus).width(2.0), c.text)
            }
            widget::text_input::Status::Disabled => (c.surface_raised, base, c.text_subdued),
        };
        widget::text_input::Style {
            background: background.into(),
            border,
            icon: c.text_subdued,
            placeholder: c.text_subdued,
            value,
            selection: c.accent.scale_alpha(0.4),
        }
    }
}

/// Pill-shaped search field in the top bar.
pub fn pill_input(
    p: &Palette,
) -> impl Fn(&Theme, widget::text_input::Status) -> widget::text_input::Style + 'static {
    let base = text_input(p);
    move |theme, status| {
        let mut style = base(theme, status);
        style.border = style.border.rounded(PILL);
        style
    }
}

/// Seek and volume bars: a thin rail that turns accent and shows its handle on hover.
pub fn slider(
    p: &Palette,
) -> impl Fn(&Theme, widget::slider::Status) -> widget::slider::Style + 'static {
    let c = p.colors;
    move |_, status| {
        let engaged: bool = status != widget::slider::Status::Active;
        widget::slider::Style {
            rail: widget::slider::Rail {
                backgrounds: (
                    (if engaged { c.accent } else { c.text }).into(),
                    mix(c.surface, c.text, 0.22).into(),
                ),
                width: 4.0,
                border: Border::default().rounded(2.0),
            },
            handle: widget::slider::Handle {
                shape: widget::slider::HandleShape::Circle {
                    radius: if engaged { 6.0 } else { 0.0 },
                },
                background: c.text.into(),
                border_width: 0.0,
                border_color: Color::TRANSPARENT,
            },
        }
    }
}

/// Thin overlay-style scrollbar without a visible rail.
pub fn scrollbar(
    p: &Palette,
) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style + 'static {
    let c = p.colors;
    move |theme, status| {
        let alpha: f32 = match status {
            scrollable::Status::Active { .. } => 0.3,
            scrollable::Status::Hovered { .. } => 0.5,
            scrollable::Status::Dragged { .. } => 0.7,
        };
        let rail = scrollable::Rail {
            background: None,
            border: Border::default(),
            scroller: scrollable::Scroller {
                background: c.text.scale_alpha(alpha).into(),
                border: Border::default().rounded(4.0),
            },
        };
        scrollable::Style {
            container: container::Style::default(),
            vertical_rail: rail,
            horizontal_rail: rail,
            gap: None,
            ..scrollable::default(theme, status)
        }
    }
}

/// Divider line between sections.
pub fn divider(p: &Palette) -> impl Fn(&Theme) -> rule::Style + 'static {
    let c = p.colors;
    move |_| rule::Style {
        color: c.border,
        radius: 0.0.into(),
        fill_mode: rule::FillMode::Full,
        snap: true,
    }
}

/// Theme outline for cards, panels and inputs.
fn outline(p: &Palette, radius: f32) -> Border {
    Border {
        color: p.colors.border,
        width: p.shape.border_width,
        radius: radius.into(),
    }
}

/// Round back/forward arrow on a raised disc; disabled arrows fade.
pub fn history_button(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let p: Palette = *p;
    move |_, status| {
        let c = p.colors;
        let (background, text_color): (Color, Color) = match status {
            button::Status::Active => (c.background, c.text),
            button::Status::Hovered | button::Status::Pressed => (c.surface_hover, c.text),
            button::Status::Disabled => (c.background, c.text_subdued.scale_alpha(0.5)),
        };
        button::Style {
            background: Some(background.into()),
            text_color,
            border: Border::default().rounded(PILL),
            shadow: Shadow::default(),
            snap: true,
        }
    }
}

/// Inline text link: subdued, turning to the text color with an underline-free hover.
pub fn link(p: &Palette) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    let c = p.colors;
    move |_, status| button::Style {
        background: None,
        text_color: match status {
            button::Status::Hovered | button::Status::Pressed => c.text,
            _ => c.text_subdued,
        },
        border: Border::default(),
        shadow: Shadow::default(),
        snap: true,
    }
}
