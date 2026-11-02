//! HEIC and AVIF decoding with macOS ImageIO (plan §5.5). ImageIO scales while
//! it decodes and applies the photo's orientation, so a 12 MP photo is never
//! held in memory at full size.

use std::ffi::c_void;

use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::{CFData, CFDataRef};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};

use crate::photos::{DecodedImage, PlatformDecoder};

#[repr(C)]
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// `kCGImageAlphaNoneSkipLast`: RGBX, 8 bits per channel.
const RGBX: u32 = 5;

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C" {
    static kCGImageSourceCreateThumbnailFromImageAlways: CFStringRef;
    static kCGImageSourceCreateThumbnailWithTransform: CFStringRef;
    static kCGImageSourceThumbnailMaxPixelSize: CFStringRef;
    fn CGImageSourceCreateWithData(data: CFDataRef, options: CFDictionaryRef) -> CFTypeRef;
    fn CGImageSourceCreateThumbnailAtIndex(
        source: CFTypeRef,
        index: usize,
        options: CFDictionaryRef,
    ) -> CFTypeRef;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    static kCGColorSpaceSRGB: CFStringRef;
    fn CGColorSpaceCreateWithName(name: CFStringRef) -> CFTypeRef;
    fn CGImageGetWidth(image: CFTypeRef) -> usize;
    fn CGImageGetHeight(image: CFTypeRef) -> usize;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CFTypeRef,
        bitmap_info: u32,
    ) -> CFTypeRef;
    fn CGContextDrawImage(context: CFTypeRef, rect: CGRect, image: CFTypeRef);
}

pub(crate) struct ImageIo;

impl PlatformDecoder for ImageIo {
    fn decode(&self, bytes: &[u8], max_edge: u32) -> Result<DecodedImage, String> {
        let data = CFData::from_buffer(bytes);
        // SAFETY: each call gets valid Core Foundation objects, and every object
        // returned under the create rule is checked for null and then owned by a
        // `CFType`, which releases it.
        unsafe {
            let source = owned(CGImageSourceCreateWithData(
                data.as_concrete_TypeRef(),
                std::ptr::null(),
            ))
            .ok_or("ImageIO can't read this file")?;
            let yes = CFBoolean::true_value().as_CFType();
            let options = CFDictionary::from_CFType_pairs(&[
                (
                    CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailFromImageAlways),
                    yes.clone(),
                ),
                (
                    CFString::wrap_under_get_rule(kCGImageSourceCreateThumbnailWithTransform),
                    yes,
                ),
                (
                    CFString::wrap_under_get_rule(kCGImageSourceThumbnailMaxPixelSize),
                    CFNumber::from(i64::from(max_edge)).as_CFType(),
                ),
            ]);
            let image = owned(CGImageSourceCreateThumbnailAtIndex(
                source.as_CFTypeRef(),
                0,
                options.as_concrete_TypeRef(),
            ))
            .ok_or("ImageIO couldn't decode this photo")?;

            let width = CGImageGetWidth(image.as_CFTypeRef());
            let height = CGImageGetHeight(image.as_CFTypeRef());
            let mut rgba = vec![0u8; width * height * 4];
            let space = owned(CGColorSpaceCreateWithName(kCGColorSpaceSRGB))
                .ok_or("no sRGB colour space")?;
            let context = owned(CGBitmapContextCreate(
                rgba.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                space.as_CFTypeRef(),
                RGBX,
            ))
            .ok_or("couldn't create a bitmap for the photo")?;
            let rect = CGRect {
                x: 0.0,
                y: 0.0,
                width: width as f64,
                height: height as f64,
            };
            CGContextDrawImage(context.as_CFTypeRef(), rect, image.as_CFTypeRef());
            drop(context);

            // The skipped fourth byte is undefined; the pixels are opaque.
            for pixel in rgba.as_chunks_mut::<4>().0 {
                pixel[3] = 255;
            }
            Ok(DecodedImage {
                width: width as u32,
                height: height as u32,
                rgba,
            })
        }
    }
}

/// Takes ownership of an object returned under the create rule.
unsafe fn owned(object: CFTypeRef) -> Option<CFType> {
    // SAFETY: the caller passes an object it owns, or null.
    (!object.is_null()).then(|| unsafe { CFType::wrap_under_create_rule(object) })
}
