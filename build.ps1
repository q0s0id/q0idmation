param(
    [ValidateSet("debug", "release", "check", "verify", "run")]
    [string]$Mode = "debug"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 3.0

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$OutDir = Join-Path $ScriptDir "out"
$LogDir = Join-Path $OutDir "build-logs"
$Stamp = Get-Date -Format "yyyyMMdd-HHmmss"
$LogPath = Join-Path $LogDir "q0editor-$Mode-$Stamp.log"
$SummaryPath = Join-Path $OutDir "last-build.txt"
$Stopwatch = [Diagnostics.Stopwatch]::StartNew()
$Failure = $null
$ArtifactPath = $null

function Find-Tool {
    param(
        [string]$Name,
        [string[]]$FallbackPaths = @()
    )

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) {
        return $command.Source
    }

    foreach ($candidate in $FallbackPaths) {
        if ($candidate -and (Test-Path -LiteralPath $candidate -PathType Leaf)) {
            return $candidate
        }
    }

    $searchPatterns = @(
        "$env:USERPROFILE\.rustup\toolchains\*\bin\$Name.exe",
        "$env:USERPROFILE\.codex\*\toolchains\*\bin\$Name.exe",
        "$env:USERPROFILE\.codex\*\bin\$Name.exe"
    )
    foreach ($pattern in $searchPatterns) {
        $match = Get-ChildItem -Path $pattern -File -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending |
            Select-Object -First 1
        if ($match) {
            return $match.FullName
        }
    }

    throw "$Name was not found. Install the stable Rust toolchain from rustup.rs."
}

function Step {
    param([string]$Text)
    Write-Host "[>] $Text" -ForegroundColor Cyan
}

function Invoke-CargoStep {
    param(
        [string]$Label,
        [string[]]$Arguments
    )

    Step $Label
    Add-Content -LiteralPath $LogPath -Value "`r`n> cargo $($Arguments -join ' ')"

    $oldErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $script:Cargo @Arguments 2>&1 |
            Out-File -LiteralPath $LogPath -Append -Encoding utf8
        $code = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $oldErrorActionPreference
    }

    if ($code -ne 0) {
        throw "cargo exited with code $code during: $Label"
    }
}

function Write-Summary {
    param(
        [string]$Status,
        [string]$Message
    )

    $lines = @(
        "q0editor build report",
        "status: $Status",
        "mode: $Mode",
        "time: $([math]::Round($Stopwatch.Elapsed.TotalSeconds, 1)) s",
        "message: $Message",
        "artifact: $ArtifactPath",
        "log: $LogPath"
    )
    Set-Content -LiteralPath $SummaryPath -Value $lines -Encoding utf8
}

try {
    if (-not (Test-Path -LiteralPath (Join-Path $ScriptDir "Cargo.toml") -PathType Leaf)) {
        throw "Cargo.toml is missing next to build.ps1"
    }
    if (-not (Test-Path -LiteralPath (Join-Path $ScriptDir "Cargo.lock") -PathType Leaf)) {
        throw "Cargo.lock is missing; locked builds are required"
    }

    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    Set-Content -LiteralPath $LogPath -Value "q0editor build log`r`nmode: $Mode`r`nstarted: $(Get-Date -Format o)" -Encoding utf8

    Step "checking rust tools"
    $Cargo = Find-Tool -Name "cargo" -FallbackPaths @("$env:USERPROFILE\.cargo\bin\cargo.exe")
    $Rustc = Find-Tool -Name "rustc" -FallbackPaths @("$env:USERPROFILE\.cargo\bin\rustc.exe")

    $toolDirs = @((Split-Path -Parent $Cargo), (Split-Path -Parent $Rustc)) | Select-Object -Unique
    foreach ($toolDir in $toolDirs) {
        if ($toolDir -and ($env:Path -notlike "*$toolDir*")) {
            $env:Path = "$toolDir;$env:Path"
        }
    }

    $cargoVersion = (& $Cargo --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw "cargo --version failed" }
    $rustVersion = (& $Rustc --version | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw "rustc --version failed" }
    Add-Content -LiteralPath $LogPath -Value "cargo: $cargoVersion`r`nrustc: $rustVersion"

    Push-Location $ScriptDir
    try {
        switch ($Mode) {
            "check" {
                Invoke-CargoStep -Label "checking q0editor" -Arguments @(
                    "check", "-p", "q0editor", "--locked"
                )
            }
            "verify" {
                Invoke-CargoStep -Label "checking formatting" -Arguments @(
                    "fmt", "--all", "--", "--check"
                )
                Invoke-CargoStep -Label "running q0editor tests" -Arguments @(
                    "test", "-p", "q0editor", "--locked"
                )
                Invoke-CargoStep -Label "running clippy" -Arguments @(
                    "clippy", "-p", "q0editor", "--all-targets", "--locked", "--", "-D", "warnings"
                )
                Invoke-CargoStep -Label "building q0editor debug" -Arguments @(
                    "build", "-p", "q0editor", "--locked"
                )
                $ArtifactPath = Join-Path $ScriptDir "target\debug\q0editor.exe"
            }
            "release" {
                $ArtifactPath = Join-Path $ScriptDir "target\release\q0editor.exe"
                $running = @(Get-Process -Name "q0editor" -ErrorAction SilentlyContinue)
                foreach ($process in $running) {
                    try {
                        if ($process.Path -and
                            [IO.Path]::GetFullPath($process.Path).Equals(
                                [IO.Path]::GetFullPath($ArtifactPath),
                                [StringComparison]::OrdinalIgnoreCase
                            )) {
                            throw "q0editor.exe is running from target\release. Save your work and close it first."
                        }
                    } catch [System.ComponentModel.Win32Exception] {
                        # Process path can be unavailable; the linker will report a useful error if needed.
                    }
                }
                Invoke-CargoStep -Label "building q0editor release" -Arguments @(
                    "build", "-p", "q0editor", "--release", "--locked"
                )
            }
            default {
                $ArtifactPath = Join-Path $ScriptDir "target\debug\q0editor.exe"
                $running = @(Get-Process -Name "q0editor" -ErrorAction SilentlyContinue)
                foreach ($process in $running) {
                    try {
                        if ($process.Path -and
                            [IO.Path]::GetFullPath($process.Path).Equals(
                                [IO.Path]::GetFullPath($ArtifactPath),
                                [StringComparison]::OrdinalIgnoreCase
                            )) {
                            throw "q0editor.exe is running from target\debug. Save your work and close it first."
                        }
                    } catch [System.ComponentModel.Win32Exception] {
                        # Process path can be unavailable; the linker will report a useful error if needed.
                    }
                }
                Invoke-CargoStep -Label "building q0editor debug" -Arguments @(
                    "build", "-p", "q0editor", "--locked"
                )
            }
        }
    } finally {
        Pop-Location
    }

    if ($ArtifactPath) {
        if (-not (Test-Path -LiteralPath $ArtifactPath -PathType Leaf)) {
            throw "build finished but q0editor.exe is missing: $ArtifactPath"
        }

        $artifact = Get-Item -LiteralPath $ArtifactPath
        $sizeMb = [math]::Round($artifact.Length / 1MB, 2)
        Add-Content -LiteralPath $LogPath -Value "`r`nartifact: $ArtifactPath`r`nsize: $sizeMb MB"
    }

    if ($Mode -eq "run") {
        Step "starting q0editor"
        Start-Process -FilePath $ArtifactPath -WorkingDirectory $ScriptDir
    }

    $Stopwatch.Stop()
    Write-Summary -Status "ok" -Message "vse norm"

    Write-Host ""
    Write-Host "[ok] vse norm" -ForegroundColor Green
    Write-Host "     mode: $Mode"
    if ($ArtifactPath) {
        Write-Host "     exe:  $ArtifactPath"
    }
    Write-Host "     time: $([math]::Round($Stopwatch.Elapsed.TotalSeconds, 1)) s"
    Write-Host "     log:  $LogPath"
} catch {
    $Failure = $_
    $Stopwatch.Stop()

    if (-not (Test-Path -LiteralPath $LogDir)) {
        New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    }
    if (-not (Test-Path -LiteralPath $LogPath)) {
        Set-Content -LiteralPath $LogPath -Value "q0editor build failed before logging started" -Encoding utf8
    }

    Add-Content -LiteralPath $LogPath -Value "`r`nFAIL: $($Failure.Exception.Message)"
    Write-Summary -Status "fail" -Message $Failure.Exception.Message

    Write-Host ""
    Write-Host "[fail] chto-to ne tak" -ForegroundColor Red
    Write-Host "       $($Failure.Exception.Message)" -ForegroundColor Red
    Write-Host ""
    Write-Host "last log lines:" -ForegroundColor Yellow
    Get-Content -LiteralPath $LogPath -Tail 35 | ForEach-Object { Write-Host "  $_" }
    Write-Host ""
    Write-Host "full log: $LogPath"
    exit 1
}
