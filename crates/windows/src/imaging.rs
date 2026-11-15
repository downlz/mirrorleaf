//! HEIC and AVIF photos with the Windows Imaging Component (WIC), ImageIO's
//! role on the Mac (plan §5.5). WIC reads them only when Microsoft's HEIF
//! Image Extensions (and, for HEIC, HEVC Video Extensions) are installed;
//! otherwise decoding fails and the app says the photo can't be read here.
//!
//! The picture is scaled while it is decoded, so a 12 MP photo is never held
//! at full size. HEIF readers apply the photo's rotation themselves (its
//! `irot` property), so no EXIF orientation is applied here.

use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppRGBA, IWICImagingFactory, IWICPalette,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};

/// 8-bit RGBA pixels, upright.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes `bytes`, scaled so that neither side is over `max_edge`.
pub fn decode(bytes: &[u8], max_edge: u32) -> windows::core::Result<Pixels> {
    // SAFETY: plain COM calls; `bytes` outlives the stream that reads it.
    unsafe {
        // Already initialised on this thread is fine too.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let stream = factory.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder = factory.CreateDecoderFromStream(
            &stream,
            std::ptr::null(),
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let (mut width, mut height) = (0u32, 0u32);
        frame.GetSize(&mut width, &mut height)?;
        let (scaled_width, scaled_height) = fit(width, height, max_edge);
        let scaler = factory.CreateBitmapScaler()?;
        scaler.Initialize(
            &frame,
            scaled_width,
            scaled_height,
            WICBitmapInterpolationModeFant,
        )?;
        let converter = factory.CreateFormatConverter()?;
        converter.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppRGBA,
            WICBitmapDitherTypeNone,
            None::<&IWICPalette>,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let (mut width, mut height) = (0u32, 0u32);
        converter.GetSize(&mut width, &mut height)?;
        let mut rgba = vec![0u8; width as usize * height as usize * 4];
        converter.CopyPixels(std::ptr::null(), width * 4, &mut rgba)?;
        Ok(Pixels {
            width,
            height,
            rgba,
        })
    }
}

/// The size that fits `max_edge`, keeping the shape; never larger.
fn fit(width: u32, height: u32, max_edge: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    if longest <= max_edge {
        return (width.max(1), height.max(1));
    }
    let scale = f64::from(max_edge) / f64::from(longest);
    (
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn photos_are_fitted_without_growing() {
        assert_eq!(super::fit(4032, 3024, 2048), (2048, 1536));
        assert_eq!(super::fit(3024, 4032, 320), (240, 320));
        assert_eq!(super::fit(100, 50, 2048), (100, 50));
    }
}
