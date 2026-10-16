param(
    [ValidateSet("check", "apk", "install", "run")]
    [string]$Mode = "apk",
    [ValidateSet("all", "editor", "player")]
    [string]$App = "all"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 3.0

$AndroidDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoDir = Split-Path -Parent $AndroidDir
$Abi = "arm64-v8a"
# Keep in step with minSdk in app\build.gradle.kts and player\build.gradle.kts.
$MinSdk = "26"
$AllApps = @(
    @{ Name = "q0editor"; Crate = "q0editor-android"; Module = "app"; Id = "com.q0id.q0editor" },
    @{ Name = "q0player"; Crate = "q0player-android"; Module = "player"; Id = "com.q0id.q0player" }
)
$Apps = @($AllApps | Where-Object { $App -eq "all" -or $_.Name -eq "q0$App" })
foreach ($target in $Apps) {
    $target.JniLibs = Join-Path $AndroidDir "$($target.Module)\src\main\jniLibs"
    $target.Apk = Join-Path $AndroidDir "$($target.Module)\build\outputs\apk\debug\$($target.Module)-debug.apk"
}
$Stopwatch = [Diagnostics.Stopwatch]::StartNew()

function Step {
    param([string]$Text)
    Write-Host "[>] $Text" -ForegroundColor Cyan
}

function Find-Newest {
    param(
        [string]$What,
        [string[]]$Patterns
    )

    foreach ($pattern in $Patterns) {
        if (-not $pattern) { continue }
        $match = Get-Item -Path $pattern -ErrorAction SilentlyContinue |
            Sort-Object Name -Descending |
            Select-Object -First 1
        if ($match) { return $match.FullName }
    }
    throw "$What was not found."
}

function Find-RustTool {
    param([string]$Name)

    $command = Get-Command $Name -ErrorAction SilentlyContinue
    if ($command) { return $command.Source }
    return Find-Newest -What "$Name (install the stable Rust toolchain and cargo-ndk)" -Patterns @(
        "$env:USERPROFILE\.cargo\bin\$Name.exe",
        "$env:USERPROFILE\.rustup\toolchains\*\bin\$Name.exe",
        "$env:USERPROFILE\.codex\*\toolchains\*\bin\$Name.exe",
        "$env:USERPROFILE\.codex\*\bin\$Name.exe"
    )
}

function Invoke-Tool {
    param(
        [string]$Label,
        [string]$Exe,
        [string[]]$Arguments
    )

    Step $Label
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed with exit code $LASTEXITCODE"
    }
}

try {
    Step "checking tools"
    $Cargo = Find-RustTool "cargo"
    $Rustc = Find-RustTool "rustc"
    $CargoNdk = Find-RustTool "cargo-ndk"
    $env:Path = "$(Split-Path -Parent $Cargo);$(Split-Path -Parent $Rustc);$env:Path;$(Split-Path -Parent $CargoNdk)"
    # cargo-ndk looks for rustc beside its own executable before PATH. With
    # rustup that is a proxy, which has to be told where the toolchain lives
    # when Rust is not installed in the default location.
    if (-not $env:CARGO_HOME) {
        $env:CARGO_HOME = Split-Path -Parent (Split-Path -Parent $CargoNdk)
    }
    if ($Rustc -match '^(?<home>.+)\\toolchains\\(?<toolchain>[^\\]+)\\bin\\') {
        if (-not $env:RUSTUP_HOME) { $env:RUSTUP_HOME = $Matches['home'] }
        if (-not $env:RUSTUP_TOOLCHAIN) { $env:RUSTUP_TOOLCHAIN = $Matches['toolchain'] }
    }

    $Sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } else {
        Find-Newest -What "Android SDK (set ANDROID_HOME)" -Patterns @(
            "$env:LOCALAPPDATA\Android\Sdk",
            "${env:ProgramFiles(x86)}\Android\android-sdk"
        )
    }
    $Ndk = if ($env:ANDROID_NDK_HOME) { $env:ANDROID_NDK_HOME } else {
        Find-Newest -What "Android NDK (set ANDROID_NDK_HOME)" -Patterns @(
            "$Sdk\ndk\*",
            "${env:ProgramFiles(x86)}\Android\AndroidNDK\*"
        )
    }
    # This Gradle release cannot run on JDK 24 or newer, so a 21 or 17 install
    # wins over whatever JAVA_HOME points at.
    $Jdk = Find-Newest -What "JDK 17 or 21 for Gradle" -Patterns @(
        "$env:ProgramFiles\Android\openjdk\jdk-21*",
        "$env:ProgramFiles\Eclipse Adoptium\jdk-21*",
        "$env:ProgramFiles\Microsoft\jdk-21*",
        "$env:ProgramFiles\Android\openjdk\jdk-17*",
        "$env:ProgramFiles\Eclipse Adoptium\jdk-17*",
        "$env:ProgramFiles\Microsoft\jdk-17*",
        "$env:ProgramFiles\Android\Android Studio\jbr"
    )
    $env:ANDROID_HOME = $Sdk
    $env:ANDROID_NDK_HOME = $Ndk
    $env:JAVA_HOME = $Jdk
    $Adb = Join-Path $Sdk "platform-tools\adb.exe"

    Push-Location $RepoDir
    try {
        foreach ($target in $Apps) {
            if ($Mode -eq "check") {
                Invoke-Tool "checking $($target.Name) for $Abi" $Cargo @(
                    "ndk", "-t", $Abi, "--platform", $MinSdk,
                    "check", "-p", $target.Crate, "--locked"
                )
            } else {
                Invoke-Tool "building the $($target.Name) native library for $Abi" $Cargo @(
                    "ndk", "-t", $Abi, "--platform", $MinSdk, "-o", $target.JniLibs,
                    "build", "-p", $target.Crate, "--release", "--locked"
                )
            }
        }
    } finally {
        Pop-Location
    }

    if ($Mode -ne "check") {
        Push-Location $AndroidDir
        try {
            $tasks = @($Apps | ForEach-Object { ":$($_.Module):assembleDebug" })
            Invoke-Tool "packaging the apks" (Join-Path $AndroidDir "gradlew.bat") (
                $tasks + @("--console=plain", "--quiet")
            )
        } finally {
            Pop-Location
        }
        foreach ($target in $Apps) {
            if (-not (Test-Path -LiteralPath $target.Apk -PathType Leaf)) {
                throw "gradle finished but the apk is missing: $($target.Apk)"
            }
        }
    }

    if ($Mode -in @("install", "run")) {
        foreach ($target in $Apps) {
            Invoke-Tool "installing $($target.Name) on the connected device" $Adb @(
                "install", "-r", $target.Apk
            )
        }
    }
    if ($Mode -eq "run") {
        # With both built, the editor is the one to start.
        $first = $Apps[0]
        Invoke-Tool "starting $($first.Name)" $Adb @(
            "shell", "am", "start", "-n", "$($first.Id)/.MainActivity"
        )
    }

    $Stopwatch.Stop()
    Write-Host ""
    Write-Host "[ok] vse norm" -ForegroundColor Green
    Write-Host "     mode: $Mode"
    Write-Host "     abi:  $Abi"
    if ($Mode -ne "check") {
        foreach ($target in $Apps) {
            $sizeMb = [math]::Round((Get-Item -LiteralPath $target.Apk).Length / 1MB, 1)
            Write-Host "     apk:  $($target.Apk) ($sizeMb MB)"
        }
    }
    Write-Host "     time: $([math]::Round($Stopwatch.Elapsed.TotalSeconds, 1)) s"
} catch {
    $Stopwatch.Stop()
    Write-Host ""
    Write-Host "[fail] chto-to ne tak" -ForegroundColor Red
    Write-Host "       $($_.Exception.Message)" -ForegroundColor Red
    exit 1
}
