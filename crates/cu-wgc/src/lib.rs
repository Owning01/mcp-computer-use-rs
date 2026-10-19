// cu-wgc — captura Windows.Graphics.Capture (WinRT) in-process.
// Port del helper C# native/wgc/Program.cs del origen (sin spawns de dotnet/csc).
// API acordada con cu-native::capture (la integra el puente en F6):
//   capture(hwnd, timeout_ms) -> Result<Frame, String>; Frame { bgra (top-down, BGRA, fila * width * 4), width, height }
// Errores con prefijo "wgc_" (p.ej. wgc_unavailable, wgc_timeout, wgc_invalid_window, wgc_error).
// PORT NOTE: el helper C# por defecto captura el cursor desactivado (--cursor 0); se replica (IsCursorCaptureEnabled=false).

use std::thread;
use std::time::{Duration, Instant};

use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::WinRT::Direct3D11::CreateDirect3D11DeviceFromDXGIDevice;
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoGetActivationFactory, RoInitialize};
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, IsWindowVisible};
use windows::core::{HSTRING, Interface};

pub struct Frame {
    pub bgra: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

pub fn capture(hwnd: i64, timeout_ms: u64) -> Result<Frame, String> {
    // PORT NOTE: RoInitialize es idempotente por thread; se ignora el error de modo cambiado.
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }

    if hwnd <= 0 {
        return Err("wgc_invalid_window".to_string());
    }
    let hwnd = HWND(hwnd as *mut core::ffi::c_void);

    // Validacion de ventana (paridad con helper C#: IsWindow / IsIconic / IsWindowVisible).
    unsafe {
        if !IsWindow(Some(hwnd)).as_bool() {
            return Err("wgc_invalid_window".to_string());
        }
        if IsIconic(hwnd).as_bool() {
            return Err("wgc_invalid_window".to_string());
        }
        if !IsWindowVisible(hwnd).as_bool() {
            return Err("wgc_invalid_window".to_string());
        }
    }

    // Soporte WGC.
    match GraphicsCaptureSession::IsSupported() {
        Ok(true) => {}
        _ => return Err("wgc_unavailable".to_string()),
    }

    // Crear GraphicsCaptureItem via interop (CreateForWindow).
    let item: GraphicsCaptureItem = {
        let interop: IGraphicsCaptureItemInterop = unsafe {
            RoGetActivationFactory(&HSTRING::from(
                "Windows.Graphics.Capture.GraphicsCaptureItem",
            ))
            .map_err(|_| "wgc_unavailable".to_string())?
        };
        unsafe {
            interop
                .CreateForWindow::<GraphicsCaptureItem>(hwnd)
                .map_err(|_| "wgc_invalid_window".to_string())?
        }
    };

    let item_size = item.Size().map_err(|_| "wgc_error".to_string())?;
    if item_size.Width <= 0 || item_size.Height <= 0 {
        return Err("wgc_invalid_window".to_string());
    }

    // Dispositivo D3D11 (hardware; fallback WARP), con BGRA support.
    let (d3d_device, context, direct3d_device) = create_d3d_device()?;

    let size = item_size;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
        &direct3d_device,
        DirectXPixelFormat::B8G8R8A8UIntNormalized,
        2,
        size,
    )
    .map_err(|_| "wgc_error".to_string())?;

    let session = pool
        .CreateCaptureSession(&item)
        .map_err(|_| "wgc_error".to_string())?;

    // Comportamiento por defecto del helper C#: cursor desactivado.
    let _ = session.SetIsCursorCaptureEnabled(false);

    session
        .StartCapture()
        .map_err(|_| "wgc_error".to_string())?;

    // Esperar primer frame con timeout (paridad: clamp 500..60000 ms).
    let timeout_ms = timeout_ms.clamp(500, 60000);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut captured_frame = None;
    while captured_frame.is_none() && Instant::now() < deadline {
        if let Ok(f) = pool.TryGetNextFrame() {
            captured_frame = Some(f);
            break;
        }
        thread::sleep(Duration::from_millis(15));
    }

    let Some(frame) = captured_frame else {
        // cleanup
        let _ = session.Close();
        let _ = pool.Close();
        return Err("wgc_timeout".to_string());
    };

    // Leer pixeles del frame via D3D11 staging texture.
    let read_result = read_frame_pixels(&d3d_device, &context, &frame);

    // cleanup
    let _ = frame.Close();
    let _ = session.Close();
    let _ = pool.Close();

    read_result
}

fn create_d3d_device() -> Result<(ID3D11Device, ID3D11DeviceContext, IDirect3DDevice), String> {
    let driver_types = [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP];
    for driver_type in driver_types {
        let mut device: Option<ID3D11Device> = None;
        let mut feature_level: D3D_FEATURE_LEVEL = D3D_FEATURE_LEVEL::default();
        let mut context: Option<ID3D11DeviceContext> = None;
        let hr = unsafe {
            D3D11CreateDevice(
                None,
                driver_type,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                Some(&mut feature_level),
                Some(&mut context),
            )
        };
        if hr.is_err() {
            continue;
        }
        let Some(d3d_device) = device else { continue };
        let Some(d3d_context) = context else { continue };

        // QI a IDXGIDevice y crear IDirect3DDevice (WinRT).
        let dxgi_device: IDXGIDevice = match d3d_device.cast() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let inspectable = match unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device) } {
            Ok(v) => v,
            Err(_) => continue,
        };
        let direct3d_device: IDirect3DDevice = match inspectable.cast() {
            Ok(v) => v,
            Err(_) => continue,
        };
        return Ok((d3d_device, d3d_context, direct3d_device));
    }
    Err("wgc_unavailable".to_string())
}

fn read_frame_pixels(
    d3d_device: &ID3D11Device,
    context: &ID3D11DeviceContext,
    frame: &windows::Graphics::Capture::Direct3D11CaptureFrame,
) -> Result<Frame, String> {
    use windows::Win32::System::WinRT::Direct3D11::IDirect3DDxgiInterfaceAccess;

    let surface = frame.Surface().map_err(|_| "wgc_error".to_string())?;
    let dxgi_access: IDirect3DDxgiInterfaceAccess =
        surface.cast().map_err(|_| "wgc_error".to_string())?;
    let texture: ID3D11Texture2D = unsafe {
        dxgi_access
            .GetInterface()
            .map_err(|_| "wgc_error".to_string())?
    };

    let mut desc = D3D11_TEXTURE2D_DESC::default();
    unsafe {
        texture.GetDesc(&mut desc);
    }
    let width = desc.Width;
    let height = desc.Height;
    if width == 0 || height == 0 {
        return Err("wgc_invalid_window".to_string());
    }

    // Staging texture (CPU read) con el formato del frame.
    let staging_desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: desc.Format,
        SampleDesc: desc.SampleDesc,
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
        MiscFlags: 0,
    };
    let mut staging: Option<ID3D11Texture2D> = None;
    unsafe {
        d3d_device
            .CreateTexture2D(&staging_desc, None, Some(&mut staging))
            .map_err(|_| "wgc_error".to_string())?;
    }
    let staging = staging.ok_or_else(|| "wgc_error".to_string())?;

    unsafe {
        context.CopyResource(&staging, &texture);
    }

    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context
            .Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|_| "wgc_error".to_string())?;
    }

    let row_bytes = (width * 4) as usize;
    let mut bgra = vec![0u8; row_bytes * height as usize];
    let src_base = mapped.pData as *const u8;
    for y in 0..height as usize {
        let src = unsafe { src_base.add(y * mapped.RowPitch as usize) };
        let dst = unsafe { bgra.as_mut_ptr().add(y * row_bytes) };
        unsafe {
            std::ptr::copy_nonoverlapping(src, dst, row_bytes);
        }
    }

    unsafe {
        context.Unmap(&staging, 0);
    }

    Ok(Frame {
        bgra,
        width,
        height,
    })
}
