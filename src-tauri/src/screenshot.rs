use windows::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{
        BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC,
        GetDIBits, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
        HBITMAP, HDC, HGDIOBJ, SRCCOPY,
    },
    UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN},
};

pub struct ScreenshotPng {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

pub fn capture_primary_display_png() -> Result<ScreenshotPng, String> {
    let width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYSCREEN) };

    if width <= 0 || height <= 0 {
        return Err("Primary display dimensions are unavailable.".to_string());
    }

    let capture = capture_bgra(width, height)?;
    let png = encode_png(width as u32, height as u32, &capture)?;

    Ok(ScreenshotPng {
        file_name: "assistant-screenshot.png".to_string(),
        bytes: png,
    })
}

fn capture_bgra(width: i32, height: i32) -> Result<Vec<u8>, String> {
    let screen_dc = ScreenDc::new()?;
    let memory_dc = MemoryDc::new(screen_dc.0)?;
    let bitmap = Bitmap::new(screen_dc.0, width, height)?;
    let previous = unsafe { SelectObject(memory_dc.0, HGDIOBJ(bitmap.0 .0)) };

    unsafe {
        BitBlt(
            memory_dc.0,
            0,
            0,
            width,
            height,
            Some(screen_dc.0),
            0,
            0,
            SRCCOPY,
        )
        .map_err(|error| format!("Screen capture failed: {error}"))?;
    }

    let mut info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
    let copied = unsafe {
        GetDIBits(
            memory_dc.0,
            bitmap.0,
            0,
            height as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        )
    };

    unsafe {
        SelectObject(memory_dc.0, previous);
    }

    if copied == 0 {
        return Err("Failed to read captured screen pixels.".to_string());
    }

    Ok(pixels)
}

fn encode_png(width: u32, height: u32, bgra: &[u8]) -> Result<Vec<u8>, String> {
    let mut rgba = Vec::with_capacity(bgra.len());

    for pixel in bgra.chunks_exact(4) {
        rgba.push(pixel[2]);
        rgba.push(pixel[1]);
        rgba.push(pixel[0]);
        rgba.push(255);
    }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| format!("Failed to initialize PNG encoder: {error}"))?;
        writer
            .write_image_data(&rgba)
            .map_err(|error| format!("Failed to encode screenshot PNG: {error}"))?;
    }

    Ok(png_bytes)
}

struct ScreenDc(HDC);

impl ScreenDc {
    fn new() -> Result<Self, String> {
        let dc = unsafe { GetDC(Some(HWND::default())) };

        if dc.is_invalid() {
            Err("Failed to acquire screen device context.".to_string())
        } else {
            Ok(Self(dc))
        }
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseDC(Some(HWND::default()), self.0);
        }
    }
}

struct MemoryDc(HDC);

impl MemoryDc {
    fn new(source: HDC) -> Result<Self, String> {
        let dc = unsafe { CreateCompatibleDC(Some(source)) };

        if dc.is_invalid() {
            Err("Failed to create capture device context.".to_string())
        } else {
            Ok(Self(dc))
        }
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

struct Bitmap(HBITMAP);

impl Bitmap {
    fn new(dc: HDC, width: i32, height: i32) -> Result<Self, String> {
        let bitmap = unsafe { CreateCompatibleBitmap(dc, width, height) };

        if bitmap.is_invalid() {
            Err("Failed to create capture bitmap.".to_string())
        } else {
            Ok(Self(bitmap))
        }
    }
}

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(HGDIOBJ(self.0 .0));
        }
    }
}
