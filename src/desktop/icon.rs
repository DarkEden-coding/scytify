//! Platform-neutral icon animation. A platform supplies an [`IconSink`] that puts a rendered
//! frame on the Dock, taskbar or tray; this module decides which frame and when.
use crate::{
    app_icon::Rgba,
    dock_icon::{DockIconRenderer, IconState},
    domain::AppState,
};
use std::time::{Duration, Instant};
use tokio::{sync::watch, time::MissedTickBehavior};

/// Edge length of the frames handed to sinks. Large enough for a 2x Dock icon.
pub const FRAME_SIZE: u32 = 256;
const FRAME_INTERVAL: Duration = Duration::from_millis(50);

/// Destination for rendered frames. `show` must not block: it runs on the animation task.
pub trait IconSink: Send + 'static {
    /// Fraction of the frame the art spans; the rest is transparent margin. See
    /// [`DockIconRenderer::new`].
    const ART_SCALE: f32 = 1.0;

    fn show(&mut self, icon: Rgba);
}

/// Show the icon matching playback until the state channel closes. Frames are produced only
/// while playing; a still state is rendered once per change.
pub async fn animate<S: IconSink>(mut state: watch::Receiver<AppState>, mut sink: S) {
    let renderer = DockIconRenderer::new(FRAME_SIZE, S::ART_SCALE);
    let started = Instant::now();
    let mut ticker = tokio::time::interval(FRAME_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut shown: Option<IconState> = None;
    let mut frame_due = true;
    loop {
        let current = IconState::from_app(&state.borrow_and_update());
        if shown != Some(current) || (current == IconState::Playing && frame_due) {
            sink.show(renderer.render(current, started.elapsed().as_secs_f32()));
            shown = Some(current);
        }
        frame_due = false;
        if current == IconState::Playing {
            tokio::select! {
                _ = ticker.tick() => frame_due = true,
                changed = state.changed() => if changed.is_err() { return },
            }
        } else if state.changed().await.is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::PlaybackStatus;
    use std::sync::{Arc, Mutex};

    struct Recorder(Arc<Mutex<Vec<Vec<u8>>>>);
    impl IconSink for Recorder {
        fn show(&mut self, icon: Rgba) {
            self.0.lock().unwrap().push(icon.pixels);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn animates_only_while_playing() {
        let (tx, rx) = watch::channel(crate::backend::initial_state());
        let frames = Arc::new(Mutex::new(Vec::new()));
        let task = tokio::spawn(animate(rx, Recorder(frames.clone())));
        let count = || frames.lock().unwrap().len();

        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(count(), 1, "a still icon is drawn once");

        tx.send_modify(|s| s.player.status = PlaybackStatus::Playing);
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(count() > 10, "playing draws a frame per tick");
        {
            let frames = frames.lock().unwrap();
            assert_ne!(frames[5], frames[6]);
        }

        tx.send_modify(|s| s.player.status = PlaybackStatus::Paused);
        tokio::time::sleep(Duration::from_millis(10)).await;
        let settled = count();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert_eq!(count(), settled, "pausing stops the frames");

        drop(tx);
        task.await.unwrap();
    }
}
