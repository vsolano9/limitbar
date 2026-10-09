# Limitbar command provider: how full a drive is, as a usage bar.
#
#   { "provider": "command", "label": "C:", "name": "Drive C",
#     "command": ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass",
#                 "-File", "%USERPROFILE%\\limitbar\\disk-space.ps1", "-Drive", "C"],
#     "poll_seconds": 600 }
#
# Prints Limitbar's provider format on standard output (docs/providers.md).
# Works in Windows PowerShell 5.1 and PowerShell 7.
param([string]$Drive = 'C')

$ErrorActionPreference = 'Stop'
try {
    $d = Get-PSDrive -Name $Drive -PSProvider FileSystem
    $used = [math]::Round($d.Used / 1GB, 1)
    $total = [math]::Round(($d.Used + $d.Free) / 1GB, 1)
    $report = @{
        windows = @(@{ key = 'disk'; tag = 'disk'; name = "Drive ${Drive}:"; used = $used; limit = $total; unit = 'GB' })
        note    = "{0} GB free" -f [math]::Round($d.Free / 1GB, 1)
    }
} catch {
    $report = @{ error = "drive ${Drive}: not found" }
}
$report | ConvertTo-Json -Depth 4 -Compress
