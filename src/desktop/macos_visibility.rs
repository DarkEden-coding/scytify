//! Whether any Scytify window can be seen. AppKit reports occlusion for the whole app, so a
//! hidden or minimized app, one on another Space and one fully covered all count as not visible.
use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2_app_kit::{
    NSApplication, NSApplicationDidChangeOcclusionStateNotification, NSApplicationOcclusionState,
};
use objc2_foundation::NSNotificationCenter;
use std::sync::OnceLock;
use tokio::sync::watch;

static VISIBLE: OnceLock<watch::Sender<bool>> = OnceLock::new();

/// Follows occlusion changes, starting from visible until AppKit reports otherwise.
pub fn receiver() -> watch::Receiver<bool> {
    VISIBLE
        .get_or_init(|| {
            DispatchQueue::main().exec_async(observe);
            watch::channel(true).0
        })
        .subscribe()
}

fn observe() {
    let Some(main) = MainThreadMarker::new() else {
        return;
    };
    let changed = block2::RcBlock::new(move |_| publish(main));
    // SAFETY: AppKit posts this notification on the main thread, where a nil queue runs the
    // block, and the block captures nothing that must cross threads.
    let observer = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidChangeOcclusionStateNotification),
            None,
            None,
            &changed,
        )
    };
    // The observer lasts as long as the process.
    std::mem::forget(observer);
    publish(main);
}

fn publish(main: MainThreadMarker) {
    let visible = NSApplication::sharedApplication(main)
        .occlusionState()
        .contains(NSApplicationOcclusionState::Visible);
    if let Some(sender) = VISIBLE.get() {
        sender.send_if_modified(|shown| std::mem::replace(shown, visible) != visible);
    }
}
