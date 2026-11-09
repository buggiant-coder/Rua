param(
    [Parameter(Mandatory=$true)][string]$Plan
)
$ErrorActionPreference = "Stop"
$planPath = (Resolve-Path -LiteralPath $Plan).Path
$updatePlan = Get-Content -LiteralPath $planPath -Raw | ConvertFrom-Json
$installed = [IO.Path]::GetFullPath($updatePlan.installed)
$working = Split-Path -Parent $planPath
$previous = Join-Path $working "previous"
$failed = Join-Path $working "failed"
$resultPath = [IO.Path]::GetFullPath($updatePlan.result)
$desktopName = [IO.Path]::GetFileName($updatePlan.desktop)
$backupReady = $false
$installationStarted = $false
$installationCommitted = $false
$parentExited = $false
$canLaunch = $false
$publisher = $null
$registrySnapshot = @()

function Write-Atomic([string]$path, [string]$text) {
    $temporary = "$path.$([Guid]::NewGuid().ToString('N')).tmp"
    $bytes = [Text.Encoding]::UTF8.GetBytes($text)
    $file = [IO.FileStream]::new($temporary, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
    try { $file.Write($bytes,0,$bytes.Length); $file.Flush($true) } finally { $file.Dispose() }
    if (Test-Path -LiteralPath $path) { [IO.File]::Replace($temporary,$path,$null) }
    else { [IO.File]::Move($temporary,$path) }
}

function Write-Result([bool]$ok, [string]$message) {
    $result = @{ success=$ok; error=$message; version=$updatePlan.version; previous_version=$updatePlan.previous_version } | ConvertTo-Json -Compress
    Write-Atomic $resultPath $result
}

function Verify-Signature([string]$path) {
    $signature = Get-AuthenticodeSignature -LiteralPath $path
    if ($signature.Status -ne "Valid") { throw "Invalid component signature: $path" }
    if ($publisher -and $signature.SignerCertificate.Subject -ne $publisher) {
        throw "Component publisher differs from installed Rua: $path"
    }
}

function Verify-Set([string]$root, [string]$version) {
    foreach ($name in @($desktopName,"rua.exe","agent-browser.exe","cua-driver.exe")) {
        $file = Join-Path $root $name
        if (-not (Test-Path -LiteralPath $file)) { throw "Missing installed component: $name" }
        Verify-Signature $file
    }
    $receipt = Get-Content -LiteralPath (Join-Path $root "automation\release-set.json") -Raw | ConvertFrom-Json
    if ($receipt.product_version -ne $version) { throw "Installed product version mismatch" }
    $verification = Start-Process -FilePath (Join-Path $root $desktopName) -ArgumentList "--verify-desktop-bundle" -PassThru -NoNewWindow
    if (-not $verification.WaitForExit(30000)) { $verification.Kill(); throw "Installed bundle verification timed out" }
    if ($verification.ExitCode -ne 0) { throw "Installed component set failed validation" }
}

function Capture-Registry([string]$path) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($path)
    if (-not $key) { return @{ path=$path; existed=$false; values=@() } }
    try {
        $values = @($key.GetValueNames() | ForEach-Object {
            @{ name=$_; kind=$key.GetValueKind($_); value=$key.GetValue($_,$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) }
        })
        return @{ path=$path; existed=$true; values=$values }
    } finally { $key.Dispose() }
}

function Restore-Registry {
    foreach ($entry in $registrySnapshot) {
        if (-not $entry.existed) {
            [Microsoft.Win32.Registry]::CurrentUser.DeleteSubKeyTree($entry.path,$false)
            continue
        }
        $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($entry.path)
        try {
            foreach ($name in $key.GetValueNames()) { $key.DeleteValue($name,$false) }
            foreach ($value in $entry.values) { $key.SetValue($value.name,$value.value,$value.kind) }
        } finally { $key.Dispose() }
    }
    # Repair only Rua's own PATH entry; keep other concurrent PATH changes.
    & "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File (Join-Path $installed "windows-path.ps1") -Action Add -InstallDir $installed
    if ($LASTEXITCODE -ne 0) { throw "Previous component files restored, but PATH repair failed" }
}

try {
    # Validate the handoff identity before acknowledging it to the old Shell.
    $parent = Get-Process -Id $updatePlan.parent_pid -ErrorAction SilentlyContinue
    if ($parent) {
        if ([IO.Path]::GetFullPath($parent.Path) -ne [IO.Path]::GetFullPath($updatePlan.desktop) -or [DateTimeOffset]::new($parent.StartTime).ToUnixTimeSeconds() -ne $updatePlan.parent_started) {
            throw "Desktop update owner identity changed; installation was not started"
        }
    }
    Write-Atomic $updatePlan.ready "ready"
    if ($parent -and -not $parent.WaitForExit(60000)) {
        throw "Rua did not finish shutdown; installation was not started"
    }
    $parentExited = $true
    $currentSignature = Get-AuthenticodeSignature -LiteralPath $updatePlan.desktop
    if ($currentSignature.Status -ne "Valid") { throw "Installed Desktop signature is invalid" }
    $publisher = $currentSignature.SignerCertificate.Subject
    if ((Get-FileHash -LiteralPath $updatePlan.installer -Algorithm SHA256).Hash.ToLowerInvariant() -ne $updatePlan.installer_sha256) { throw "Verified installer changed before execution" }
    Verify-Signature $updatePlan.installer
    Verify-Set $installed $updatePlan.previous_version
    $uninstallKey = "Software\Microsoft\Windows\CurrentVersion\Uninstall\Rua"
    $registration = Get-ItemProperty -LiteralPath "HKCU:\$uninstallKey"
    if ([IO.Path]::GetFullPath($registration.InstallLocation.Trim('"')) -ne $installed) {
        throw "Rua installer registration belongs to another installation"
    }
    $manufacturer = $registration.Publisher
    if (-not $manufacturer -or $manufacturer.Contains('\')) { throw "Invalid installed publisher registration" }
    $registrySnapshot = @(
        (Capture-Registry $uninstallKey),
        (Capture-Registry "Software\$manufacturer\Rua"),
        (Capture-Registry "Software\Rua\Desktop")
    )
    Copy-Item -LiteralPath $installed -Destination $previous -Recurse
    Verify-Set $previous $updatePlan.previous_version
    $backupReady = $true
    # /R is deliberately absent: this supervisor launches only the validated set.
    # NSIS requires /D to be last and unquoted, including paths with spaces.
    $installerProcess = Start-Process -FilePath $updatePlan.installer -ArgumentList "/S /UPDATE /D=$installed" -PassThru
    $installationStarted = $true
    if (-not $installerProcess.WaitForExit(300000)) {
        & "$env:SystemRoot\System32\taskkill.exe" /PID $installerProcess.Id /T /F | Out-Null
        if (-not $installerProcess.WaitForExit(10000)) { throw "Installer did not stop; backup preserved at $previous" }
        throw "Desktop installation timed out"
    }
    if ($installerProcess.ExitCode -ne 0) { throw "Desktop installer failed: $($installerProcess.ExitCode)" }
    Verify-Set $installed $updatePlan.version
    Write-Result $true ""
    $installationCommitted = $true
    $canLaunch = $true
} catch {
    $reason = $_.Exception.Message
    $installerSettled = -not $installationStarted -or $installerProcess.HasExited
    if ($backupReady -and -not $installationCommitted -and $installerSettled) {
        try {
            if (Test-Path -LiteralPath $installed) { Move-Item -LiteralPath $installed -Destination $failed }
            Move-Item -LiteralPath $previous -Destination $installed
            Verify-Set $installed $updatePlan.previous_version
            Restore-Registry
            $canLaunch = $true
            $reason = "$reason; previous component set restored"
        } catch {
            $reason = "$reason; rollback failed: $($_.Exception.Message); preserved files: $working"
        }
    } elseif (-not $installationStarted -and $parentExited) {
        try { Verify-Set $installed $updatePlan.previous_version; $canLaunch = $true } catch { }
    }
    Write-Result $false $reason
}
if ($canLaunch -and $parentExited -and (Test-Path -LiteralPath (Join-Path $installed $desktopName))) {
    Start-Process -FilePath (Join-Path $installed $desktopName)
    Remove-Item -LiteralPath $working -Recurse -Force -ErrorAction SilentlyContinue
}
