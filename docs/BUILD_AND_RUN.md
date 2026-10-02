# Selva Build & Run Instructions

## On machines WITH a GPU (normal PCs, laptops):
```
cargo build --release
./target/release/selva.exe
```

## On machines WITHOUT GPU (VMs, Hyper-V, headless):
Mesa3D DLLs must be placed next to the executable. Copy from `mesa3d/x64/` to `target/release/`:
- opengl32.dll
- libglapi.dll
- libgallium_wgl.dll
- libEGL.dll

Then set env vars before running:
```
set LIBGL_ALWAYS_SOFTWARE=1
set MESA_GL_VERSION_OVERRIDE=2.1
selva.exe
```

## Alternative: wgpu backend (DirectX/Vulkan instead of OpenGL)
Change Cargo.toml:
```toml
eframe = { version = "0.27", default-features = false, features = ["persistence", "wgpu", "default_fonts"] }
```
This uses DirectX on Windows — works on VMs with DirectX support.