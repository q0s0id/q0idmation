# build.ps1 - build the independent, versioned q0editor and q0player installers.
#
# The q0editor and q0player Cargo package versions are the release version.
# They must match. NSIS receives that version explicitly; version.nsh is only
# the safe fallback for a direct, manual makensis invocation.

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 3.0

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$WorkspaceDir = Split-Path -Parent $ScriptDir
$InstallerDir = $ScriptDir
$DistDir = Join-Path $InstallerDir "dist"

$AttemptDir = $null
$AttemptFiles = @()
$Failure = $null

function Have-Cmd($cmd) {
    return [bool](Get-Command $cmd -ErrorAction SilentlyContinue)
}

function Ensure-Tool {
    param(
        [string]$Name,
        [string]$Cmd,
        [string]$InstallHint,
        [string[]]$ExtraSearchPaths = @()
    )

    if (Have-Cmd $Cmd) {
        Write-Host "[ok] $Name found" -ForegroundColor DarkGreen
        return
    }

    # Fallback: a tool may be installed but not listed in PATH (NSIS often is).
    foreach ($candidate in $ExtraSearchPaths) {
        if (Test-Path -LiteralPath $candidate) {
            $toolDir = Split-Path -Parent $candidate
            $env:Path = "$toolDir;$env:Path"
            if (Have-Cmd $Cmd) {
                Write-Host "[ok] $Name found at $candidate" -ForegroundColor DarkGreen
                return
            }
        }
    }

    throw "$Name is not installed. $InstallHint"
}

function Step($message) {
    Write-Host ""
    Write-Host "==> $message" -ForegroundColor Cyan
}

function Remove-AttemptFiles {
    param(
        [string[]]$Paths,
        [AllowNull()][string]$Directory
    )

    foreach ($path in $Paths) {
        if ($path -and (Test-Path -LiteralPath $path)) {
            Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
        }
    }

    if ($Directory -and (Test-Path -LiteralPath $Directory)) {
        $remaining = @(Get-ChildItem -LiteralPath $Directory -Force -ErrorAction SilentlyContinue)
        if ($remaining.Count -eq 0) {
            Remove-Item -LiteralPath $Directory -Force -ErrorAction SilentlyContinue
        } else {
            Write-Host "[warn] Preserved unexpected files in $Directory" -ForegroundColor Yellow
        }
    }
}

try {
    # -------- 0. Tools and release identity --------

    Step "Checking tools"
    Ensure-Tool -Name "Python" -Cmd "python" `
        -InstallHint "Install Python 3.10+ from https://www.python.org/downloads/."
    Ensure-Tool -Name "Cargo" -Cmd "cargo" `
        -InstallHint "Install Rust via https://rustup.rs/." `
        -ExtraSearchPaths @("$env:USERPROFILE\.cargo\bin\cargo.exe")
    Ensure-Tool -Name "Rust compiler" -Cmd "rustc" `
        -InstallHint "Install Rust via https://rustup.rs/." `
        -ExtraSearchPaths @("$env:USERPROFILE\.cargo\bin\rustc.exe")
    Ensure-Tool -Name "NSIS" -Cmd "makensis" `
        -InstallHint "Install NSIS 3.x from https://nsis.sourceforge.io/Download." `
        -ExtraSearchPaths @(
            "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
            "$env:ProgramFiles\NSIS\makensis.exe"
        )

    Push-Location $WorkspaceDir
    try {
        $metadataJson = & cargo metadata --no-deps --format-version 1
        if ($LASTEXITCODE -ne 0) {
            throw "cargo metadata exited with $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
    $metadata = (($metadataJson | Out-String).Trim() | ConvertFrom-Json)
    $editorPackages = @($metadata.packages | Where-Object { $_.name -eq "q0editor" })
    $playerPackages = @($metadata.packages | Where-Object { $_.name -eq "q0player" })
    if ($editorPackages.Count -ne 1 -or $playerPackages.Count -ne 1) {
        throw "cargo metadata must contain exactly one q0editor and one q0player package"
    }

    $EditorVersion = [string]$editorPackages[0].version
    $PlayerVersion = [string]$playerPackages[0].version
    if ($EditorVersion -ne $PlayerVersion) {
        throw "package version mismatch: q0editor=$EditorVersion, q0player=$PlayerVersion"
    }
    $BetaVersion = $EditorVersion

    $semverMatch = [regex]::Match(
        $BetaVersion,
        '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$'
    )
    if (-not $semverMatch.Success) {
        throw "Cargo package version is not valid SemVer: $BetaVersion"
    }
    $numericParts = @(
        [uint64]$semverMatch.Groups[1].Value,
        [uint64]$semverMatch.Groups[2].Value,
        [uint64]$semverMatch.Groups[3].Value
    )
    if (@($numericParts | Where-Object { $_ -gt 65535 }).Count -gt 0) {
        throw "version components must fit the Windows 0..65535 version fields: $BetaVersion"
    }
    $NumericVersion = "{0}.{1}.{2}.0" -f $numericParts[0], $numericParts[1], $numericParts[2]
    $ArtifactVersion = $BetaVersion -replace '[^0-9A-Za-z.-]', '_'

    $RustVersion = ((& rustc --version) | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw "rustc --version failed" }
    $CargoVersion = ((& cargo --version) | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw "cargo --version failed" }
    $RustVerbose = @(& rustc -vV)
    if ($LASTEXITCODE -ne 0) { throw "rustc -vV failed" }
    $RustHostLine = @($RustVerbose | Where-Object { $_ -like "host: *" }) | Select-Object -First 1
    if (-not $RustHostLine) { throw "rustc -vV did not report a host triple" }
    $RustHostTriple = $RustHostLine.Substring(6).Trim()
    if ($RustHostTriple -ne "x86_64-pc-windows-msvc") {
        throw "this packaging script produces Windows x64 artifacts; Rust host is $RustHostTriple"
    }
    $NsisVersion = ((& makensis /VERSION) | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $NsisVersion) { throw "makensis /VERSION failed" }

    $BuildUtc = [DateTime]::UtcNow
    $BuildUtcText = $BuildUtc.ToString(
        "yyyy-MM-dd'T'HH:mm:ss'Z'",
        [Globalization.CultureInfo]::InvariantCulture
    )
    $BuildStamp = $BuildUtc.ToString(
        "yyyyMMdd'T'HHmmss'Z'",
        [Globalization.CultureInfo]::InvariantCulture
    )
    $BuildId = "q0s-$ArtifactVersion-windows-x64-$BuildStamp"

    Write-Host "[ok] beta version $BetaVersion" -ForegroundColor DarkGreen
    Write-Host "[ok] Rust host $RustHostTriple" -ForegroundColor DarkGreen

    New-Item -ItemType Directory -Force -Path $DistDir | Out-Null

    # These two old, unversioned names are deliberately retired. Remove only
    # these exact files so they cannot be mistaken for the current beta.
    foreach ($deprecatedName in @("q0editor-setup.exe", "q0player-setup.exe")) {
        $deprecatedPath = Join-Path $DistDir $deprecatedName
        if (Test-Path -LiteralPath $deprecatedPath) {
            Remove-Item -LiteralPath $deprecatedPath -Force
            Write-Host "[clean] removed deprecated $deprecatedName"
        }
    }

    $AttemptName = ".attempt-$PID-$([Guid]::NewGuid().ToString('N'))"
    $AttemptDir = Join-Path $DistDir $AttemptName
    New-Item -ItemType Directory -Path $AttemptDir | Out-Null

    $InstallerSpecs = @(
        [pscustomobject]@{
            App = "q0player"
            Script = "q0player-setup.nsi"
            FileName = "q0player-$ArtifactVersion-windows-x64-setup.exe"
        },
        [pscustomobject]@{
            App = "q0editor"
            Script = "q0editor-setup.nsi"
            FileName = "q0editor-$ArtifactVersion-windows-x64-setup.exe"
        }
    )
    foreach ($spec in $InstallerSpecs) {
        $spec | Add-Member -NotePropertyName StagedPath -NotePropertyValue (Join-Path $AttemptDir $spec.FileName)
        $spec | Add-Member -NotePropertyName FinalPath -NotePropertyValue (Join-Path $DistDir $spec.FileName)
    }
    $StagedShaPath = Join-Path $AttemptDir "SHA256SUMS.txt"
    $StagedBuildInfoPath = Join-Path $AttemptDir "BUILD-INFO.txt"
    $FinalShaPath = Join-Path $DistDir "SHA256SUMS.txt"
    $FinalBuildInfoPath = Join-Path $DistDir "BUILD-INFO.txt"
    $AttemptFiles = @($InstallerSpecs | ForEach-Object { $_.StagedPath }) + @(
        $StagedShaPath,
        $StagedBuildInfoPath
    )

    # -------- 1. Verification gate --------

    Step "Running beta verification gate"
    Push-Location $WorkspaceDir
    try {
        $releaseTargetDir = Join-Path $WorkspaceDir "target"

        & cargo fmt --all -- --check
        if ($LASTEXITCODE -ne 0) {
            throw "cargo fmt check exited with $LASTEXITCODE"
        }

        & cargo test --workspace --locked --target-dir $releaseTargetDir
        if ($LASTEXITCODE -ne 0) {
            throw "cargo test exited with $LASTEXITCODE"
        }

        & cargo clippy --workspace --all-targets --locked --target-dir $releaseTargetDir -- -D warnings
        if ($LASTEXITCODE -ne 0) {
            throw "cargo clippy exited with $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }

    # -------- 2. Assets --------

    Step "Generating installer assets (icons + BMP)"
    & python "$InstallerDir\build_assets.py"
    if ($LASTEXITCODE -ne 0) {
        throw "build_assets.py exited with $LASTEXITCODE"
    }

    # -------- 3. Cargo release build --------

    Step "Building release binaries"

    # A running app holds its release EXE open and blocks the linker.
    $busy = @()
    foreach ($name in @("q0editor", "q0player")) {
        $process = Get-Process -Name $name -ErrorAction SilentlyContinue
        if ($process) { $busy += $name }
    }
    if ($busy.Count -gt 0) {
        Write-Host "[warn] These processes are running and would block the linker:" -ForegroundColor Yellow
        foreach ($name in $busy) { Write-Host "         $name.exe" -ForegroundColor Yellow }
        $response = Read-Host "Close them now? [y/N]"
        if ($response -match '^(y|Y)') {
            foreach ($name in $busy) {
                Get-Process -Name $name -ErrorAction SilentlyContinue | Stop-Process -Force
            }
            Start-Sleep -Milliseconds 500
        } else {
            throw "close q0editor and q0player, then rerun the build"
        }
    }

    Push-Location $WorkspaceDir
    try {
        # Pin the directory so inherited CARGO_TARGET_DIR cannot redirect the
        # build while NSIS still reads workspace\target\release.
        & cargo build --release --locked -p q0editor -p q0player --target-dir $releaseTargetDir
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build exited with $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }

    $editorExe = Join-Path $WorkspaceDir "target\release\q0editor.exe"
    $playerExe = Join-Path $WorkspaceDir "target\release\q0player.exe"
    foreach ($exe in @($editorExe, $playerExe)) {
        if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) {
            throw "expected release binary is missing: $exe"
        }
    }

    # -------- 4. NSIS --------

    Step "Building versioned installers"
    Push-Location $InstallerDir
    try {
        foreach ($spec in $InstallerSpecs) {
            $relativeOutput = "dist\$AttemptName\$($spec.FileName)"
            Write-Host "  makensis $($spec.Script) -> $($spec.FileName)"
            & makensis /V2 `
                "/DAPP_VERSION=$BetaVersion" `
                "/DAPP_VERSION_NUMERIC=$NumericVersion" `
                "/DOUTPUT_FILE=$relativeOutput" `
                $spec.Script
            if ($LASTEXITCODE -ne 0) {
                throw "makensis $($spec.Script) exited with $LASTEXITCODE"
            }
        }
    } finally {
        Pop-Location
    }

    $ArtifactRecords = @()
    foreach ($spec in $InstallerSpecs) {
        if (-not (Test-Path -LiteralPath $spec.StagedPath -PathType Leaf)) {
            throw "expected installer is missing: $($spec.StagedPath)"
        }
        $item = Get-Item -LiteralPath $spec.StagedPath
        $hash = (Get-FileHash -LiteralPath $spec.StagedPath -Algorithm SHA256).Hash.ToUpperInvariant()
        $ArtifactRecords += [pscustomobject]@{
            FileName = $spec.FileName
            StagedPath = $spec.StagedPath
            FinalPath = $spec.FinalPath
            Size = [int64]$item.Length
            Sha256 = $hash
        }
    }

    $shaLines = @($ArtifactRecords | ForEach-Object { "$($_.Sha256)  $($_.FileName)" })
    Set-Content -LiteralPath $StagedShaPath -Value ($shaLines -join "`r`n") -Encoding Ascii

    $buildInfoLines = @(
        "Q0S beta build",
        "Build ID: $BuildId",
        "Version: $BetaVersion",
        "Build UTC: $BuildUtcText",
        "Platform: Windows x64",
        "Rust host: $RustHostTriple",
        "Rust: $RustVersion",
        "Cargo: $CargoVersion",
        "NSIS: $NsisVersion",
        "",
        "Artifacts:"
    )
    foreach ($artifact in $ArtifactRecords) {
        $buildInfoLines += $artifact.FileName
        $buildInfoLines += "  Size: $($artifact.Size) bytes"
        $buildInfoLines += "  SHA256: $($artifact.Sha256)"
    }
    Set-Content -LiteralPath $StagedBuildInfoPath -Value ($buildInfoLines -join "`r`n") -Encoding Ascii

    # Publish only after every artifact and manifest is complete. Existing
    # files of this same version are backed up and restored if publication
    # fails; other versions in dist are never touched.
    $PublishPairs = @(
        @($ArtifactRecords | ForEach-Object {
            [pscustomobject]@{ Staged = $_.StagedPath; Final = $_.FinalPath }
        }) + @(
            [pscustomobject]@{ Staged = $StagedShaPath; Final = $FinalShaPath },
            [pscustomobject]@{ Staged = $StagedBuildInfoPath; Final = $FinalBuildInfoPath }
        )
    )
    $Published = @()
    $Backups = @()
    try {
        foreach ($pair in $PublishPairs) {
            if (Test-Path -LiteralPath $pair.Final) {
                $backupName = ".previous-$([Guid]::NewGuid().ToString('N'))-$([IO.Path]::GetFileName($pair.Final))"
                $backupPath = Join-Path $AttemptDir $backupName
                Move-Item -LiteralPath $pair.Final -Destination $backupPath
                $Backups += [pscustomobject]@{ Backup = $backupPath; Final = $pair.Final }
            }
            Move-Item -LiteralPath $pair.Staged -Destination $pair.Final
            $Published += $pair.Final
        }
    } catch {
        for ($index = $Published.Count - 1; $index -ge 0; $index--) {
            if (Test-Path -LiteralPath $Published[$index]) {
                Remove-Item -LiteralPath $Published[$index] -Force -ErrorAction SilentlyContinue
            }
        }
        for ($index = $Backups.Count - 1; $index -ge 0; $index--) {
            $backup = $Backups[$index]
            if (Test-Path -LiteralPath $backup.Backup) {
                Move-Item -LiteralPath $backup.Backup -Destination $backup.Final -Force
            }
        }
        throw
    }
    foreach ($backup in $Backups) {
        if (Test-Path -LiteralPath $backup.Backup) {
            Remove-Item -LiteralPath $backup.Backup -Force
        }
    }

    Step "Done"
    Write-Host "    Build ID: $BuildId"
    foreach ($artifact in $ArtifactRecords) {
        $sizeMb = [math]::Round($artifact.Size / 1MB, 2)
        Write-Host ("    {0}  ({1} bytes, {2} MB)" -f $artifact.FinalPath, $artifact.Size, $sizeMb)
        Write-Host ("      SHA256 {0}" -f $artifact.Sha256)
    }
    Write-Host "    $FinalShaPath"
    Write-Host "    $FinalBuildInfoPath"
} catch {
    $Failure = $_
} finally {
    Remove-AttemptFiles -Paths $AttemptFiles -Directory $AttemptDir
}

if ($Failure) {
    Write-Host "[fail] $($Failure.Exception.Message)" -ForegroundColor Red
    exit 1
}
