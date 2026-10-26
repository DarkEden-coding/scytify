//! Dock icon sink: replaces the running app's icon image through AppKit on the main queue.
use super::icon::IconSink;
use crate::app_icon::Rgba;
use dispatch2::DispatchQueue;
use image::{
    ExtendedColorType, ImageEncoder,
    codecs::png::{CompressionType, FilterType, PngEncoder},
};
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{NSApplication, NSImage};
use objc2_foundation::NSData;

pub struct DockIcon;

impl IconSink for DockIcon {
    // AppKit draws a runtime icon image edge to edge, while the bundle icon is inset into the
    // system icon grid; 824 of 1024 is Apple's template size for the art.
    const ART_SCALE: f32 = 824.0 / 1024.0;

    fn show(&mut self, icon: Rgba) {
        // Encoding happens here, off the main thread; only the AppKit calls hop over.
        let mut png = Vec::new();
        let encoded = PngEncoder::new_with_quality(&mut png, CompressionType::Fast, FilterType::NoFilter)
            .write_image(&icon.pixels, icon.width, icon.height, ExtendedColorType::Rgba8);
        if let Err(error) = encoded {
            log::warn!("Dock icon frame not encoded: {error}");
            return;
        }
        DispatchQueue::main().exec_async(move || {
            let Some(main) = MainThreadMarker::new() else {
                return;
            };
            let data = NSData::with_bytes(&png);
            if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                // SAFETY: Runs on the main thread (checked by the marker) with a valid image.
                unsafe {
                    NSApplication::sharedApplication(main).setApplicationIconImage(Some(&image));
                }
            }
        });
    }
}
