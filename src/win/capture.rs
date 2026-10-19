//! Window capture: real Windows Graphics Capture (CreateForMonitor + crop),
//! with GDI BitBlt as fallback. Reports which path was used via `last_path()`.

use crate::engine::CaptureResult;
use crate::geometry::{self, CaptureGeometry, Rect};
use crate::image;
use crate::win::shell;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::{Interface, HSTRING};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;
use windows_core::BOOL;

static LAST_PATH: Mutex<CapturePathInfo> = Mutex::new(CapturePathInfo {
    path: String::new(),
    detail: String::new(),
});

#[derive(Clone, Default)]
struct CapturePathInfo {
    path: String,
    detail: String,
}

/// `wgc` or `bitblt` (empty before first capture).
pub fn last_path() -> String {
    LAST_PATH.lock().map(|g| g.path.clone()).unwrap_or_default()
}

pub fn last_path_detail() -> String {
    LAST_PATH.lock().map(|g| g.detail.clone()).unwrap_or_default()
}

fn set_path(path: &str, detail: impl Into<String>) {
    if let Ok(mut g) = LAST_PATH.lock() {
        g.path = path.to_string();
        g.detail = detail.into();
    }
}

/// Hardware first, then WARP. Hosted runners and many VMs have no usable GPU,
/// and WGC still needs a D3D11 device to copy the frame out.
fn create_capture_device() -> Result<
    (
        windows::Win32::Graphics::Direct3D11::ID3D11Device,
        windows::Win32::Graphics::Direct3D11::ID3D11DeviceContext,
    ),
    String,
> {
    use windows::Win32::Graphics::Direct3D::{
        D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL_11_0,
    };
    use windows::Win32::Graphics::Direct3D11::{
        D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        D3D11_SDK_VERSION,
    };
    let drivers = [(D3D_DRIVER_TYPE_HARDWARE, "hardware"), (D3D_DRIVER_TYPE_WARP, "warp")];
    let mut errors = Vec::new();
    for (driver, name) in drivers {
        let mut device: Option<ID3D11Device> = None;
        let mut context: Option<ID3D11DeviceContext> = None;
        let hr = unsafe {
            D3D11CreateDevice(
                None,
                driver,
                windows::Win32::Foundation::HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
        };
        match hr {
            Ok(()) => {
                if let (Some(d), Some(c)) = (device, context) {
                    return Ok((d, c));
                }
                errors.push(format!("{name}: device missing"));
            }
            Err(e) => errors.push(format!("{name}: {e}")),
        }
    }
    Err(format!("D3D11CreateDevice failed ({})", errors.join("; ")))
}

pub fn capture_window(hwnd: HWND, max_edge: u32) -> Result<CaptureResult, String> {
    let bounds = shell::window_rect(hwnd)?;
    if bounds.is_empty() {
        return Err("window has invalid bounds or is not visible".into());
    }
    match capture_wgc(&bounds, max_edge) {
        Ok(r) => {
            set_path("wgc", "Windows Graphics Capture (CreateForMonitor + crop)");
            Ok(r)
        }
        Err(wgc_err) => match capture_bitblt(&bounds, max_edge) {
            Ok(r) => {
                set_path("bitblt", format!("WGC unavailable or timed out ({wgc_err}); used GDI BitBlt"));
                Ok(r)
            }
            Err(e) => {
                set_path("failed", format!("wgc: {wgc_err}; bitblt: {e}"));
                Err(format!("capture failed (wgc: {wgc_err}; bitblt: {e})"))
            }
        },
    }
}

fn capture_bitblt(bounds: &Rect, max_edge: u32) -> Result<CaptureResult, String> {
    unsafe {
        let w = bounds.width();
        let h = bounds.height();
        if w <= 0 || h <= 0 {
            return Err("empty bounds".into());
        }
        let hdc_screen = GetDC(Some(GetDesktopWindow()));
        if hdc_screen.0.is_null() {
            return Err("GetDC failed".into());
        }
        let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
        let hbmp = CreateCompatibleBitmap(hdc_screen, w, h);
        let old = SelectObject(hdc_mem, HGDIOBJ(hbmp.0));
        let ok = BitBlt(hdc_mem, 0, 0, w, h, Some(hdc_screen), bounds.left, bounds.top, SRCCOPY);
        if ok.is_err() {
            SelectObject(hdc_mem, old);
            let _ = DeleteObject(HGDIOBJ(hbmp.0));
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(Some(GetDesktopWindow()), hdc_screen);
            return Err("BitBlt failed".into());
        }
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0 as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
        let got = GetDIBits(
            hdc_mem,
            hbmp,
            0,
            h as u32,
            Some(bgra.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );
        SelectObject(hdc_mem, old);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        let _ = DeleteDC(hdc_mem);
        ReleaseDC(Some(GetDesktopWindow()), hdc_screen);
        if got == 0 {
            return Err("GetDIBits failed".into());
        }
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }
        finish_capture(*bounds, w as u32, h as u32, &bgra, max_edge)
    }
}

fn finish_capture(window: Rect, src_w: u32, src_h: u32, bgra: &[u8], max_edge: u32) -> Result<CaptureResult, String> {
    let (iw, ih, scale) = geometry::fit_size(src_w, src_h, max_edge);
    let pixels = if iw == src_w && ih == src_h {
        bgra.to_vec()
    } else {
        image::resize_bgra_nn(src_w, src_h, bgra, iw, ih)?
    };
    let png = image::encode_bgra_png(iw, ih, &pixels)?;
    Ok(CaptureResult {
        png_data_url: image::png_data_url(&png),
        geometry: CaptureGeometry {
            window,
            scale,
            image_w: iw,
            image_h: ih,
        },
        screenshot_id: String::new(),
    })
}

fn capture_wgc(bounds: &Rect, max_edge: u32) -> Result<CaptureResult, String> {
    let monitors = enum_monitors();
    let mon = geometry::dominant_monitor(bounds, &monitors)
        .ok_or_else(|| "no monitor found for window".to_string())?;
    let crop = geometry::crop_in_monitor(bounds, mon)?;
    wgc_grab(mon, &crop, bounds, max_edge)
}

fn enum_monitors() -> Vec<Rect> {
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
    let mut out: Vec<Rect> = Vec::new();
    unsafe extern "system" fn cb(mon: HMONITOR, _hdc: HDC, _r: *mut RECT, lp: LPARAM) -> BOOL {
        let list = &mut *(lp.0 as *mut Vec<Rect>);
        let mut mi = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(mon, &mut mi).as_bool() {
            list.push(Rect::new(
                mi.rcMonitor.left,
                mi.rcMonitor.top,
                mi.rcMonitor.right,
                mi.rcMonitor.bottom,
            ));
        }
        BOOL(1)
    }
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    if out.is_empty() {
        out.push(crate::win::input::virtual_desktop());
    }
    out
}

fn monitor_handle(r: &Rect) -> Option<windows::Win32::Graphics::Gdi::HMONITOR> {
    use windows::Win32::Graphics::Gdi::{MonitorFromRect, MONITOR_DEFAULTTONEAREST};
    let rect = RECT {
        left: r.left,
        top: r.top,
        right: r.right,
        bottom: r.bottom,
    };
    let h = unsafe { MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST) };
    if h.0.is_null() {
        None
    } else {
        Some(h)
    }
}

const WGC_FRAME_TIMEOUT: Duration = Duration::from_millis(1500);

fn wgc_grab(monitor: &Rect, crop: &Rect, window: &Rect, max_edge: u32) -> Result<CaptureResult, String> {
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Graphics::SizeInt32;
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Resource, ID3D11Texture2D, D3D11_BIND_FLAG, D3D11_CPU_ACCESS_FLAG, D3D11_CPU_ACCESS_READ,
        D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::System::WinRT::Direct3D11::{
        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
    };
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::System::WinRT::{RoGetActivationFactory, RoInitialize, RO_INIT_SINGLETHREADED};

    unsafe {
        // The calling thread is already STA (UIA CoInitializeEx). MTA here returns
        // RPC_E_CHANGED_MODE and can leave WinRT capture unusable.
        let _ = RoInitialize(RO_INIT_SINGLETHREADED);
        let (d3d, ctx) = create_capture_device()?;

        let dxgi: IDXGIDevice = d3d.cast().map_err(|e| format!("IDXGIDevice cast: {e}"))?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
            .map_err(|e| format!("CreateDirect3D11DeviceFromDXGIDevice: {e}"))?;
        let winrt_device: IDirect3DDevice = inspectable
            .cast()
            .map_err(|e| format!("IDirect3DDevice cast: {e}"))?;

        let hmon = monitor_handle(monitor).ok_or("monitor handle unavailable")?;
        let interop: IGraphicsCaptureItemInterop =
            RoGetActivationFactory(&HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureItem"))
                .map_err(|e| format!("RoGetActivationFactory: {e}"))?;
        let item: GraphicsCaptureItem = interop
            .CreateForMonitor(hmon)
            .map_err(|e| format!("CreateForMonitor: {e}"))?;
        let size = item.Size().map_err(|e| format!("GraphicsCaptureItem.Size: {e}"))?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err("capture item has invalid size".into());
        }

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            1,
            SizeInt32 {
                Width: size.Width,
                Height: size.Height,
            },
        )
        .map_err(|e| format!("Direct3D11CaptureFramePool.CreateFreeThreaded: {e}"))?;

        let session = pool
            .CreateCaptureSession(&item)
            .map_err(|e| format!("CreateCaptureSession: {e}"))?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        // IsBorderRequired is Win10 2104+; ignore failure on older builds.
        let _ = session.SetIsBorderRequired(false);
        session
            .StartCapture()
            .map_err(|e| format!("StartCapture: {e}"))?;

        let deadline = Instant::now() + WGC_FRAME_TIMEOUT;
        let frame = loop {
            match pool.TryGetNextFrame() {
                Ok(f) => break f,
                Err(_) => {
                    if Instant::now() >= deadline {
                        let _ = session.Close();
                        let _ = pool.Close();
                        return Err("window capture timed out waiting for WGC frame".into());
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };

        let surface = frame.Surface().map_err(|e| format!("frame.Surface: {e}"))?;
        let access: IDirect3DDxgiInterfaceAccess = surface
            .cast()
            .map_err(|e| format!("IDirect3DDxgiInterfaceAccess cast: {e}"))?;
        let src_tex: ID3D11Texture2D = access
            .GetInterface()
            .map_err(|e| format!("GetInterface(ID3D11Texture2D): {e}"))?;

        let mut desc = D3D11_TEXTURE2D_DESC::default();
        src_tex.GetDesc(&mut desc);
        if desc.Width == 0 || desc.Height == 0 {
            let _ = session.Close();
            let _ = pool.Close();
            return Err("captured texture has invalid size".into());
        }

        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: D3D11_BIND_FLAG(0).0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_FLAG(D3D11_CPU_ACCESS_READ.0).0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        d3d.CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .map_err(|e| format!("CreateTexture2D(staging): {e}"))?;
        let staging = staging.ok_or("CreateTexture2D returned no staging texture")?;

        let src_res: ID3D11Resource = src_tex.cast().map_err(|e| format!("src as resource: {e}"))?;
        let dst_res: ID3D11Resource = staging.cast().map_err(|e| format!("staging as resource: {e}"))?;
        ctx.CopyResource(&dst_res, &src_res);

        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&dst_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| format!("Map staging texture: {e}"))?;

        let src_w = desc.Width;
        let src_h = desc.Height;
        let pitch = mapped.RowPitch as usize;
        let mut bgra_full = vec![0u8; (src_w as usize) * (src_h as usize) * 4];
        for y in 0..src_h as usize {
            let src_row = (mapped.pData as *const u8).add(y * pitch);
            let dst_off = y * (src_w as usize) * 4;
            std::ptr::copy_nonoverlapping(src_row, bgra_full[dst_off..].as_mut_ptr(), (src_w as usize) * 4);
        }
        ctx.Unmap(&dst_res, 0);

        let _ = session.Close();
        let _ = pool.Close();
        drop(frame);

        // Crop relative to the monitor texture.
        let crop_l = crop.left.max(0) as u32;
        let crop_t = crop.top.max(0) as u32;
        let crop_w = crop.width().max(0) as u32;
        let crop_h = crop.height().max(0) as u32;
        if crop_l.saturating_add(crop_w) > src_w || crop_t.saturating_add(crop_h) > src_h {
            return Err("window crop is outside captured monitor".into());
        }
        let mut cropped = image::crop_bgra(src_w, src_h, &bgra_full, crop_l, crop_t, crop_w, crop_h)?;
        for px in cropped.chunks_exact_mut(4) {
            px[3] = 255;
        }
        finish_capture(*window, crop_w, crop_h, &cropped, max_edge)
    }
}


/// Capture for the requested input mode.
/// Background prefers CreateForWindow + PrintWindow; never activates the window.
/// Capture an entire monitor (scoped screenshot without a window).
pub fn capture_monitor(monitor: &crate::geometry::Rect, max_edge: u32) -> Result<CaptureResult, String> {
    match wgc_grab(monitor, monitor, monitor, max_edge) {
        Ok(c) => {
            set_path("wgc", "Windows Graphics Capture (CreateForMonitor full)");
            Ok(c)
        }
        Err(e) => Err(format!("scoped monitor capture failed: {e}")),
    }
}

pub fn capture_for_mode(hwnd: HWND, max_edge: u32, background: bool) -> Result<(CaptureResult, &'static str), String> {
    use windows::Win32::UI::WindowsAndMessaging::IsIconic;
    unsafe {
        if IsIconic(hwnd).as_bool() {
            return Err("window is minimized; restore it before capturing in background mode".into());
        }
    }
    if background {
        match capture_wgc_window(hwnd, max_edge) {
            Ok(r) => {
                set_path("wgc_window", "WGC CreateForWindow (background)");
                return Ok((r, "wgc_window"));
            }
            Err(e1) => match capture_printwindow(hwnd, max_edge) {
                Ok(r) => {
                    set_path("printwindow", format!("WGC window failed ({e1}); PrintWindow"));
                    return Ok((r, "printwindow"));
                }
                Err(e2) => {
                    set_path("failed", format!("wgc_window: {e1}; printwindow: {e2}"));
                    return Err(format!("background capture failed (wgc_window: {e1}; printwindow: {e2})"));
                }
            },
        }
    }
    // Foreground / auto path: monitor WGC then BitBlt.
    let r = capture_window(hwnd, max_edge)?;
    let tag: &'static str = match last_path().as_str() {
        "wgc" => "wgc",
        "bitblt" => "bitblt",
        _ => "bitblt",
    };
    Ok((r, tag))
}

fn capture_printwindow(hwnd: HWND, max_edge: u32) -> Result<CaptureResult, String> {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits,
        ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    let bounds = shell::window_rect(hwnd)?;
    let w = bounds.width();
    let h = bounds.height();
    if w <= 0 || h <= 0 {
        return Err("empty bounds".into());
    }
    unsafe {
        let hdc_win = GetDC(Some(hwnd));
        if hdc_win.0.is_null() {
            return Err("GetDC(hwnd) failed".into());
        }
        let hdc_mem = CreateCompatibleDC(Some(hdc_win));
        let hbmp = CreateCompatibleBitmap(hdc_win, w, h);
        let old = SelectObject(hdc_mem, HGDIOBJ(hbmp.0));
        // PW_RENDERFULLCONTENT = 0x00000002
        let ok = PrintWindow(hwnd, hdc_mem, PRINT_WINDOW_FLAGS(2u32));
        if ok.0 == 0 {
            SelectObject(hdc_mem, old);
            let _ = DeleteObject(HGDIOBJ(hbmp.0));
            let _ = DeleteDC(hdc_mem);
            ReleaseDC(Some(hwnd), hdc_win);
            return Err("PrintWindow failed".into());
        }
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0 as u32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bgra = vec![0u8; (w as usize) * (h as usize) * 4];
        let got = GetDIBits(hdc_mem, hbmp, 0, h as u32, Some(bgra.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        SelectObject(hdc_mem, old);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        let _ = DeleteDC(hdc_mem);
        ReleaseDC(Some(hwnd), hdc_win);
        if got == 0 {
            return Err("GetDIBits after PrintWindow failed".into());
        }
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }
        finish_capture(bounds, w as u32, h as u32, &bgra, max_edge)
    }
}

fn capture_wgc_window(hwnd: HWND, max_edge: u32) -> Result<CaptureResult, String> {
    use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem};
    use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
    use windows::Graphics::DirectX::DirectXPixelFormat;
    use windows::Graphics::SizeInt32;
    use windows::Win32::Graphics::Direct3D11::{
        ID3D11Resource, ID3D11Texture2D, D3D11_BIND_FLAG, D3D11_CPU_ACCESS_FLAG, D3D11_CPU_ACCESS_READ,
        D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    };
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    use windows::Win32::Graphics::Dxgi::IDXGIDevice;
    use windows::Win32::System::WinRT::Direct3D11::{
        CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
    };
    use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
    use windows::Win32::System::WinRT::{RoGetActivationFactory, RoInitialize, RO_INIT_SINGLETHREADED};
    use windows::core::{Interface, HSTRING};
    use std::time::{Duration, Instant};

    let bounds = shell::window_rect(hwnd)?;
    unsafe {
        let _ = RoInitialize(RO_INIT_SINGLETHREADED);
        let (d3d, ctx) = create_capture_device()?;
        let dxgi: IDXGIDevice = d3d.cast().map_err(|e| e.to_string())?;
        let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi).map_err(|e| e.to_string())?;
        let winrt_device: IDirect3DDevice = inspectable.cast().map_err(|e| e.to_string())?;
        let interop: IGraphicsCaptureItemInterop =
            RoGetActivationFactory(&HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureItem"))
                .map_err(|e| e.to_string())?;
        let item: GraphicsCaptureItem = interop.CreateForWindow(hwnd).map_err(|e| format!("CreateForWindow: {e}"))?;
        let size = item.Size().map_err(|e| e.to_string())?;
        if size.Width <= 0 || size.Height <= 0 {
            return Err("capture item has invalid size".into());
        }
        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            1,
            SizeInt32 { Width: size.Width, Height: size.Height },
        )
        .map_err(|e| e.to_string())?;
        let session = pool.CreateCaptureSession(&item).map_err(|e| e.to_string())?;
        let _ = session.SetIsCursorCaptureEnabled(false);
        let _ = session.SetIsBorderRequired(false);
        session.StartCapture().map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_millis(1500);
        let frame = loop {
            match pool.TryGetNextFrame() {
                Ok(f) => break f,
                Err(_) if Instant::now() >= deadline => {
                    let _ = session.Close();
                    let _ = pool.Close();
                    return Err("CreateForWindow frame timed out".into());
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        };
        let surface = frame.Surface().map_err(|e| e.to_string())?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(|e| e.to_string())?;
        let src_tex: ID3D11Texture2D = access.GetInterface().map_err(|e| e.to_string())?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        src_tex.GetDesc(&mut desc);
        let staging_desc = D3D11_TEXTURE2D_DESC {
            Width: desc.Width,
            Height: desc.Height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_STAGING,
            BindFlags: D3D11_BIND_FLAG(0).0 as u32,
            CPUAccessFlags: D3D11_CPU_ACCESS_FLAG(D3D11_CPU_ACCESS_READ.0).0 as u32,
            MiscFlags: 0,
        };
        let mut staging: Option<ID3D11Texture2D> = None;
        d3d.CreateTexture2D(&staging_desc, None, Some(&mut staging)).map_err(|e| e.to_string())?;
        let staging = staging.ok_or("no staging")?;
        let src_res: ID3D11Resource = src_tex.cast().map_err(|e| e.to_string())?;
        let dst_res: ID3D11Resource = staging.cast().map_err(|e| e.to_string())?;
        ctx.CopyResource(&dst_res, &src_res);
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&dst_res, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(|e| e.to_string())?;
        let src_w = desc.Width;
        let src_h = desc.Height;
        let pitch = mapped.RowPitch as usize;
        let mut bgra = vec![0u8; (src_w as usize) * (src_h as usize) * 4];
        for y in 0..src_h as usize {
            let src_row = (mapped.pData as *const u8).add(y * pitch);
            let dst_off = y * (src_w as usize) * 4;
            std::ptr::copy_nonoverlapping(src_row, bgra[dst_off..].as_mut_ptr(), (src_w as usize) * 4);
        }
        ctx.Unmap(&dst_res, 0);
        let _ = session.Close();
        let _ = pool.Close();
        for px in bgra.chunks_exact_mut(4) {
            px[3] = 255;
        }
        // Client-area crop: WGC window capture is typically the window outer size;
        // we already used DWM extended bounds as `bounds`.
        finish_capture(bounds, src_w, src_h, &bgra, max_edge)
    }
}
