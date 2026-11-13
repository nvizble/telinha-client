param([switch]$DebugBuild)
$ErrorActionPreference = "Stop"
Set-Location (Split-Path $PSScriptRoot -Parent)

if (-not $env:GPUI_FXC_PATH) {
    $fxc = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\fxc.exe" |
        Sort-Object FullName -Descending | Select-Object -First 1
    if (-not $fxc) { throw "Instale o Windows SDK com o fxc.exe (Visual Studio Build Tools)." }
    $env:GPUI_FXC_PATH = $fxc.FullName
}
if ($env:GITHUB_ENV) {
    "GPUI_FXC_PATH=$env:GPUI_FXC_PATH" | Out-File -FilePath $env:GITHUB_ENV -Encoding utf8 -Append
}
$cargoArgs = @("build", "--locked", "-p", "telinha", "--target", "x86_64-pc-windows-msvc")
$profile = "debug"
if (-not $DebugBuild) { $cargoArgs += "--release"; $profile = "release" }
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw "A compilação falhou." }
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }
New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item "$targetDir/x86_64-pc-windows-msvc/$profile/telinha.exe" dist/telinha.exe
$probe = Start-Process -FilePath (Resolve-Path dist/telinha.exe) -ArgumentList "--camera-probe" -Wait -PassThru
if ($probe.ExitCode -ne 0) { throw "O processo auxiliar de webcam falhou no teste sem hardware." }
(Get-FileHash dist/telinha.exe -Algorithm SHA256).Hash.ToLower() + "  telinha.exe" |
    Set-Content dist/telinha.exe.sha256 -Encoding ascii
Write-Host "Pronto: dist/telinha.exe"
