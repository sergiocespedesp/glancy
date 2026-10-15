//! Windows Imaging Component: decodes embedded previews (JPEG, TIFF, PNG…) and saves PNG
//! (the latter only for the command-line tool and the tests).

use crate::image::Image;
use windows::Win32::Foundation::GENERIC_WRITE;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_ContainerFormatPng, GUID_WICPixelFormat32bppBGRA, GUID_WICPixelFormat32bppRGBA,
    IWICImagingFactory, WICBitmapEncoderNoCache, WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::{HSTRING, Result};

/// Initializes COM on the current thread if needed (tools and tests; inside Explorer it already is).
pub fn ensure_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn factory() -> Result<IWICImagingFactory> {
    unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
}

pub const MAX_PIXELS: u64 = 32 * 1024 * 1024;

/// First frame of an in-memory image, as RGBA with straight alpha.
pub fn decode(bytes: &[u8]) -> Result<Image> {
    unsafe {
        let f = factory()?;
        let stream = f.CreateStream()?;
        stream.InitializeFromMemory(bytes)?;
        let decoder = f.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let rgba = WICConvertBitmapSource(&GUID_WICPixelFormat32bppRGBA, &frame)?;
        let (mut w, mut h) = (0u32, 0u32);
        rgba.GetSize(&mut w, &mut h)?;
        // Embedded previews are small; a cap protects Explorer's memory (32 Mpx = 128 MB).
        if w == 0 || h == 0 || (w as u64) * (h as u64) > MAX_PIXELS {
            return Err(windows::core::Error::new(windows::Win32::Foundation::E_FAIL, "preview with invalid size"));
        }
        let mut data = vec![0u8; w as usize * h as usize * 4];
        rgba.CopyPixels(std::ptr::null(), w * 4, &mut data)?;
        Ok(Image::from_rgba(w, h, data))
    }
}

/// Saves RGBA (straight alpha) as PNG.
pub fn save_png(img: &Image, path: &std::path::Path) -> Result<()> {
    let mut bgra = img.data.clone();
    for p in bgra.as_chunks_mut::<4>().0.iter_mut() {
        p.swap(0, 2);
    }
    unsafe {
        let f = factory()?;
        let stream = f.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let encoder = f.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
        frame.Initialize(None)?;
        frame.SetSize(img.width, img.height)?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WritePixels(img.height, img.width * 4, &bgra)?;
        frame.Commit()?;
        encoder.Commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_roundtrip() {
        ensure_com();
        let mut img = Image::new(3, 2);
        img.set_pixel(0, 0, [255, 0, 0, 255]);
        img.set_pixel(2, 1, [0, 0, 255, 128]);
        let path = std::env::temp_dir().join(format!("glancy-wic-{}.png", std::process::id()));
        save_png(&img, &path).unwrap();
        let back = decode(&std::fs::read(&path).unwrap()).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(back, img);
    }

    #[test]
    fn garbage_fails() {
        ensure_com();
        assert!(decode(b"esto no es una imagen").is_err());
    }
}
