param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Add", "Remove")]
    [string]$Action,

    [Parameter(Mandatory = $true)]
    [string]$InstallDir
)

$ErrorActionPreference = "Stop"
$markerPath = "HKCU:\Software\Rua\Desktop"
$markerName = "PathEntryOwned"
$normalizedInstallDir = [IO.Path]::GetFullPath($InstallDir).TrimEnd('\')

function Get-UserPathEntries {
    $value = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $value) {
        return @()
    }
    return @($value.Split(';') | Where-Object { $_.Trim() } | ForEach-Object {
        $_.Trim().TrimEnd('\')
    })
}

function Set-UserPathEntries([string[]]$Entries) {
    [Environment]::SetEnvironmentVariable("Path", ($Entries -join ';'), "User")
    Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(
    IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
    $result = [UIntPtr]::Zero
    [void][Win32.NativeMethods]::SendMessageTimeout(
        [IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, "Environment",
        0x0002, 5000, [ref]$result)
}

$entries = @(Get-UserPathEntries)

if ($Action -eq "Add") {
    $alreadyPresent = $entries | Where-Object {
        $_.Equals($normalizedInstallDir, [StringComparison]::OrdinalIgnoreCase)
    }
    if (-not $alreadyPresent) {
        Set-UserPathEntries @($entries + $normalizedInstallDir)
        New-Item -Path $markerPath -Force | Out-Null
        Set-ItemProperty -Path $markerPath -Name $markerName -Value $normalizedInstallDir
    }
    exit 0
}

$owned = $null
if (Test-Path $markerPath) {
    $owned = (Get-ItemProperty -Path $markerPath -Name $markerName -ErrorAction SilentlyContinue).$markerName
}
if ($owned -and $owned.Equals($normalizedInstallDir, [StringComparison]::OrdinalIgnoreCase)) {
    $remaining = @($entries | Where-Object {
        -not $_.Equals($normalizedInstallDir, [StringComparison]::OrdinalIgnoreCase)
    })
    Set-UserPathEntries $remaining
    Remove-ItemProperty -Path $markerPath -Name $markerName -ErrorAction SilentlyContinue
}
