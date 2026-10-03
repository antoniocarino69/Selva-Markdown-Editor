# Selva Build & Run Instructions

```powershell
cargo build --release
.\target\release\selva.exe
```

## Windows, Hyper-V and Remote Desktop

Windows builds use eframe's wgpu renderer with DirectX 12. Windows' software
adapter (Microsoft Basic Render Driver / WARP) can be selected automatically
when there is no suitable physical GPU. No Mesa DLLs or OpenGL environment
variables are needed for this path. Software rendering uses the CPU.

Launch `target/release/selva.exe` normally. Startup and graphics diagnostics are
written to `%LOCALAPPDATA%\Selva\logs\startup.log`, replaced on each launch.
Set `RUST_LOG=debug` before launching for more detail. Other platforms retain
eframe's default renderer.

Validated on 2026-10-02 in this Windows Hyper-V VM: the release window renders
and responds, and the startup log identifies `Microsoft Basic Render Driver
(Dx12, Cpu)`. Both the Hyper-V Video and Remote Display adapters are installed.

Reference: [Microsoft WARP documentation](https://learn.microsoft.com/en-us/windows/win32/direct3darticles/directx-warp).
