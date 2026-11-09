param(
    [Parameter(Mandatory = $true)]
    [string]$Installer
)

$ErrorActionPreference = "Stop"
$acceptanceDir = Join-Path $PWD "target\desktop-acceptance\windows"
$dataDir = Join-Path $acceptanceDir "data"
$logDir = Join-Path $acceptanceDir "logs"
New-Item -ItemType Directory -Force -Path $dataDir, $logDir | Out-Null

Start-Process -FilePath $Installer -ArgumentList "/S" -Wait

$candidateRoots = @(
    (Join-Path $env:LOCALAPPDATA "Rua"),
    (Join-Path $env:LOCALAPPDATA "Programs\Rua")
)
$installDir = $candidateRoots | Where-Object {
    (Test-Path (Join-Path $_ "rua.exe")) -and
    (Test-Path (Join-Path $_ "rua-desktop.exe"))
} | Select-Object -First 1
if (-not $installDir) {
    throw "Installed Rua directory was not found"
}

$runtime = Join-Path $installDir "rua.exe"
$desktop = Get-Item -LiteralPath (Join-Path $installDir "rua-desktop.exe")
if (-not $desktop) {
    throw "Desktop Shell executable was not found"
}
if (-not (Test-Path (Join-Path $installDir "console\index.html"))) {
    throw "Packaged Console assets were not found"
}

foreach ($component in @("rua-desktop.exe", "rua.exe", "agent-browser.exe", "cua-driver.exe")) {
    $binary = Join-Path $installDir $component
    if ((Get-AuthenticodeSignature -LiteralPath $binary).Status -ne "Valid") {
        throw "Invalid bundled component signature: $component"
    }
}
$verification = Start-Process -FilePath $desktop.FullName -ArgumentList "--verify-desktop-bundle" -PassThru
if (-not $verification.WaitForExit(30000)) { $verification.Kill(); throw "Bundle verification timed out" }
if ($verification.ExitCode -ne 0) { throw "Installed component set validation failed" }
& $runtime --version
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not ($userPath.Split(';') | Where-Object {
    $_.TrimEnd('\').Equals($installDir.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)
})) {
    throw "Installer did not add its own directory to the user PATH"
}

$env:RUA_DESKTOP_ROOT = $dataDir
$process = Start-Process -FilePath $desktop.FullName -PassThru `
    -RedirectStandardOutput (Join-Path $logDir "stdout.log") `
    -RedirectStandardError (Join-Path $logDir "stderr.log")

$endpoint = Join-Path $dataDir "desktop-endpoint.json"
for ($i = 0; $i -lt 240 -and -not (Test-Path $endpoint); $i++) {
    Start-Sleep -Milliseconds 500
}
if (-not (Test-Path $endpoint)) {
    throw "Desktop Runtime did not publish endpoint state"
}
$port = (Get-Content $endpoint -Raw | ConvertFrom-Json).port
Invoke-WebRequest "http://127.0.0.1:$port/readyz" -UseBasicParsing |
    Select-Object -ExpandProperty Content |
    Set-Content (Join-Path $acceptanceDir "readyz.json")
Invoke-WebRequest "http://127.0.0.1:$port/console/" -UseBasicParsing |
    Select-Object -ExpandProperty Content |
    Set-Content (Join-Path $acceptanceDir "console.html")

Start-Process -FilePath $desktop.FullName -ArgumentList "--quit" -Wait
$process.WaitForExit(30000)
if (-not $process.HasExited) {
    throw "Desktop Shell did not quit gracefully"
}

$uninstaller = Get-ChildItem $installDir -Filter "uninstall*.exe" | Select-Object -First 1
if (-not $uninstaller) {
    throw "NSIS uninstaller was not found"
}
Start-Process -FilePath $uninstaller.FullName -ArgumentList "/S" -Wait

if (-not (Test-Path $dataDir)) {
    throw "Uninstall deleted product data"
}
$userPathAfter = [Environment]::GetEnvironmentVariable("Path", "User")
if ($userPathAfter.Split(';') | Where-Object {
    $_.TrimEnd('\').Equals($installDir.TrimEnd('\'), [StringComparison]::OrdinalIgnoreCase)
}) {
    throw "Uninstall left the Rua-owned PATH entry behind"
}

Write-Host "Windows installed application acceptance passed"
