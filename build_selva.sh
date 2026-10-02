#!/bin/bash
export PATH="/c/Users/Administrator/.cargo/bin:$PATH"
MSVC_LINK_PATH="/c/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/Tools/MSVC/14.44.35207/bin/HostX64/x64"
# Put MSVC link.exe FIRST so it shadows git-bash's /usr/bin/link.exe
export PATH="$MSVC_LINK_PATH:$PATH"
export LIB='C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\lib\x64;C:\Program Files (x86)\Windows Kits\10\lib\10.0.26100.0\ucrt\x64;C:\Program Files (x86)\Windows Kits\10\lib\10.0.26100.0\um\x64'
export INCLUDE='C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.44.35207\include;C:\Program Files (x86)\Windows Kits\10\include\10.0.26100.0\ucrt;C:\Program Files (x86)\Windows Kits\10\include\10.0.26100.0\um;C:\Program Files (x86)\Windows Kits\10\include\10.0.26100.0\shared'
which link.exe
link.exe 2>&1 | head -2
cd /c/Projects/Selva

# Design requirement: the release exe must be fully portable ("one-shot ovunque"):
# static MSVC runtime, no VC redist / UCRT needed on the target machine.
# Belt and braces on top of .cargo/config.toml (currently untracked, could be lost).
export RUSTFLAGS="-C target-feature=+crt-static"

cargo build --release 2>&1
BUILD_STATUS=$?
if [ $BUILD_STATUS -ne 0 ]; then
    exit $BUILD_STATUS
fi

# Gate: fail the build if the exe dynamically links the VC runtime / UCRT.
DUMPBIN="$MSVC_LINK_PATH/dumpbin.exe"
DEPS=$("$DUMPBIN" /dependents target/release/selva.exe 2>/dev/null | tr '[:upper:]' '[:lower:]')
if echo "$DEPS" | grep -E 'vcruntime|msvcp[0-9]|ucrtbase|api-ms-win-crt'; then
    echo "ERROR: selva.exe dynamically links the C runtime — must be built with +crt-static"
    exit 1
fi
echo "OK: selva.exe has no C runtime dependencies (portable build)"