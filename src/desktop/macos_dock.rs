//! Dock icon sink: draws frames into the Dock tile through AppKit on the main queue. Redrawing
//! the tile is much cheaper than replacing the application icon, so the icon, which the app
//! switcher shows, changes only with the playback state.
use super::icon::IconSink;
use crate::app_icon::Rgba;
use dispatch2::DispatchQueue;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSApplication, NSBitmapFormat, NSBitmapImageRep, NSColorSpace, NSDeviceRGBColorSpace, NSImage,
    NSImageView,
};
use objc2_foundation::NSSize;

pub struct DockIcon;

thread_local! {
    /// The Dock tile's content view, created with the first frame.
    static VIEW: std::cell::RefCell<Option<objc2::rc::Retained<NSImageView>>> =
        const { std::cell::RefCell::new(None) };
}

impl IconSink for DockIcon {
    // AppKit draws a runtime icon image edge to edge, while the bundle icon is inset into the
    // system icon grid; 824 of 1024 is Apple's template size for the art.
    const ART_SCALE: f32 = 824.0 / 1024.0;

    fn show(&mut self, icon: Rgba, state_changed: bool) {
        DispatchQueue::main().exec_async(move || {
            let Some(main) = MainThreadMarker::new() else {
                return;
            };
            let Some(image) = image(&icon) else {
                return;
            };
            let app = NSApplication::sharedApplication(main);
            if state_changed {
                // SAFETY: Runs on the main thread (checked by the marker) with a valid image.
                unsafe { app.setApplicationIconImage(Some(&image)) };
            }
            let tile = app.dockTile();
            VIEW.with(|view| {
                let mut view = view.borrow_mut();
                let view = view.get_or_insert_with(|| {
                    let created = NSImageView::imageViewWithImage(&image, main);
                    tile.setContentView(Some(&created));
                    created
                });
                view.setImage(Some(&image));
            });
            tile.display();
        });
    }
}

/// Wraps the frame's pixels in an sRGB bitmap, so AppKit draws them without decoding a file.
fn image(icon: &Rgba) -> Option<objc2::rc::Retained<NSImage>> {
    let (width, height) = (icon.width as isize, icon.height as isize);
    // SAFETY: Null planes make AppKit allocate a buffer of `height` rows of `width * 4` bytes,
    // which receives exactly that many bytes below.
    let bitmap = unsafe {
        let bitmap = NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            width,
            height,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            NSBitmapFormat::AlphaNonpremultiplied,
            width * 4,
            32,
        )?;
        let pixels = bitmap.bitmapData();
        if pixels.is_null() || icon.pixels.len() != (width * height * 4) as usize {
            return None;
        }
        std::ptr::copy_nonoverlapping(icon.pixels.as_ptr(), pixels, icon.pixels.len());
        bitmap
    };
    let bitmap = bitmap.bitmapImageRepByRetaggingWithColorSpace(&NSColorSpace::sRGBColorSpace())?;
    let image = NSImage::initWithSize(
        NSImage::alloc(),
        NSSize::new(icon.width as f64 / 2.0, icon.height as f64 / 2.0),
    );
    image.addRepresentation(&bitmap);
    Some(image)
}
