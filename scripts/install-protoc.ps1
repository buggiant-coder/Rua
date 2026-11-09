param(
    [switch]$Help
)

if ($Help) {
    Write-Host "Usage: scripts/install-protoc.ps1"
    Write-Host "Installs the Protocol Buffers compiler used by prost-build."
    exit 0
}

$existing = Get-Command protoc -ErrorAction SilentlyContinue
if ($existing) {
    & $existing.Source --version
    exit 0
}

if (Get-Command winget -ErrorAction SilentlyContinue) {
    winget install --id Google.Protobuf --exact --accept-package-agreements --accept-source-agreements
} elseif (Get-Command choco -ErrorAction SilentlyContinue) {
    choco install protoc -y
} elseif (Get-Command scoop -ErrorAction SilentlyContinue) {
    scoop install protobuf
} else {
    Write-Error "No supported package manager found. Install protoc manually and ensure it is in PATH."
    exit 1
}

$installed = Get-Command protoc -ErrorAction SilentlyContinue
if (-not $installed) {
    Write-Error "protoc was installed but is not visible in PATH. Open a new shell and retry."
    exit 1
}

& $installed.Source --version
