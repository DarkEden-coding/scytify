//! Writes dock icon frames as PNG files: `render_dock_icon <closed|paused|playing> <size> <out.png> [seconds]`.
//! The `closed` render is the source of `assets/icons/scytify-closed-*.png`.
use scytify::dock_icon::{DockIconRenderer, IconState};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [state, size, out, rest @ ..] = args.as_slice() else {
        return Err(
            "usage: render_dock_icon <closed|paused|playing> <size> <out.png> [seconds]".into(),
        );
    };
    let state = match state.as_str() {
        "closed" => IconState::Closed,
        "paused" => IconState::Paused,
        "playing" => IconState::Playing,
        other => return Err(format!("unknown state {other}").into()),
    };
    let seconds: f32 = rest.first().map(|s| s.parse()).transpose()?.unwrap_or(0.0);
    let icon = DockIconRenderer::new(size.parse()?, 1.0).render(state, seconds);
    image::save_buffer(
        out,
        &icon.pixels,
        icon.width,
        icon.height,
        image::ExtendedColorType::Rgba8,
    )?;
    Ok(())
}
