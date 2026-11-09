param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath
)

$ErrorActionPreference = "Stop"

if (-not $env:WINDOWS_CERTIFICATE_THUMBPRINT) {
    throw "WINDOWS_CERTIFICATE_THUMBPRINT is required"
}

$signTool = Get-Command signtool.exe -ErrorAction SilentlyContinue
$signToolPath = if ($signTool) { $signTool.Source } else { $null }
if (-not $signToolPath) {
    $kits = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" `
        -Directory -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending
    foreach ($kit in $kits) {
        $candidate = Join-Path $kit.FullName "x64\signtool.exe"
        if (Test-Path $candidate) {
            $signToolPath = $candidate
            break
        }
    }
}
if (-not $signToolPath) {
    throw "signtool.exe was not found"
}

& $signToolPath sign `
    /sha1 $env:WINDOWS_CERTIFICATE_THUMBPRINT `
    /fd SHA256 `
    /tr "http://timestamp.digicert.com" `
    /td SHA256 `
    /v `
    $FilePath

if ($LASTEXITCODE -ne 0) {
    throw "signtool.exe failed with exit code $LASTEXITCODE"
}
