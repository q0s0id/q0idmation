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

function Find-Tool {
    param(
        [string]$Name,
        [string]$InstallHint,
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

    throw "$Name is not installed. $InstallHint"
}

function Invoke-CheckedText {
    param(
        [string]$Program,
        [string[]]$Arguments,
        [string]$Label
    )

    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        $output = & $Program @Arguments 2>&1
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
    }

    if ($exitCode -ne 0) {
        $details = (($output | Out-String).Trim())
        if ($details) {
            throw "$Label failed with exit code ${exitCode}: $details"
        }
        throw "$Label failed with exit code $exitCode"
    }

    return (($output | Out-String).Trim())
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
    $Python = Find-Tool -Name "python" `
        -InstallHint "Install Python 3.10+ from https://www.python.org/downloads/." `
        -FallbackPaths @("C:\Python310\python.exe", "$env:LOCALAPPDATA\Programs\Python\Python310\python.exe")
    $Cargo = Find-Tool -Name "cargo" `
        -InstallHint "Install the stable Rust toolchain from https://rustup.rs/." `
        -FallbackPaths @("$env:USERPROFILE\.cargo\bin\cargo.exe")
    $Rustc = Find-Tool -Name "rustc" `
        -InstallHint "Install the stable Rust toolchain from https://rustup.rs/." `
        -FallbackPaths @("$env:USERPROFILE\.cargo\bin\rustc.exe")
    $CargoFmt = Find-Tool -Name "cargo-fmt" `
        -InstallHint "Install the rustfmt component with rustup component add rustfmt." `
        -FallbackPaths @("$env:USERPROFILE\.cargo\bin\cargo-fmt.exe")
    $CargoClippy = Find-Tool -Name "cargo-clippy" `
        -InstallHint "Install the clippy component with rustup component add clippy." `
        -FallbackPaths @("$env:USERPROFILE\.cargo\bin\cargo-clippy.exe")
    $MakeNsis = Find-Tool -Name "makensis" `
        -InstallHint "Install NSIS 3.x from https://nsis.sourceforge.io/Download." `
        -FallbackPaths @(
            "${env:ProgramFiles(x86)}\NSIS\makensis.exe",
            "$env:ProgramFiles\NSIS\makensis.exe"
        )
    $Git = Find-Tool -Name "git" `
        -InstallHint "Install Git for Windows from https://git-scm.com/download/win."

    $toolDirs = @(
        (Split-Path -Parent $Cargo),
        (Split-Path -Parent $Rustc),
        (Split-Path -Parent $CargoFmt),
        (Split-Path -Parent $CargoClippy),
        (Split-Path -Parent $MakeNsis),
        (Split-Path -Parent $Git)
    ) | Select-Object -Unique
    $env:Path = (($toolDirs + @($env:Path)) -join [IO.Path]::PathSeparator)

    Write-Host "[ok] Python: $Python" -ForegroundColor DarkGreen
    Write-Host "[ok] Cargo: $Cargo" -ForegroundColor DarkGreen
    Write-Host "[ok] Rust compiler: $Rustc" -ForegroundColor DarkGreen
    Write-Host "[ok] rustfmt: $CargoFmt" -ForegroundColor DarkGreen
    Write-Host "[ok] Clippy: $CargoClippy" -ForegroundColor DarkGreen
    Write-Host "[ok] NSIS: $MakeNsis" -ForegroundColor DarkGreen
    Write-Host "[ok] Git: $Git" -ForegroundColor DarkGreen

    Push-Location $WorkspaceDir
    try {
        $metadataJson = & $Cargo metadata --no-deps --format-version 1
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

    $RustVersion = ((& $Rustc --version) | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw "rustc --version failed" }
    $CargoVersion = ((& $Cargo --version) | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) { throw "cargo --version failed" }
    $RustVerbose = @(& $Rustc -vV)
    if ($LASTEXITCODE -ne 0) { throw "rustc -vV failed" }
    $RustHostLine = @($RustVerbose | Where-Object { $_ -like "host: *" }) | Select-Object -First 1
    if (-not $RustHostLine) { throw "rustc -vV did not report a host triple" }
    $RustHostTriple = $RustHostLine.Substring(6).Trim()
    if ($RustHostTriple -ne "x86_64-pc-windows-msvc") {
        throw "this packaging script produces Windows x64 artifacts; Rust host is $RustHostTriple"
    }
    $NsisVersion = ((& $MakeNsis /VERSION) | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or -not $NsisVersion) { throw "makensis /VERSION failed" }

    Step "Checking Git release identity"
    Push-Location $WorkspaceDir
    try {
        $GitRoot = Invoke-CheckedText -Program $Git -Arguments @("rev-parse", "--show-toplevel") -Label "git root lookup"
        if ((Resolve-Path -LiteralPath $GitRoot).Path -ne (Resolve-Path -LiteralPath $WorkspaceDir).Path) {
            throw "workspace is not the Git repository root: $GitRoot"
        }

        $GitStatus = Invoke-CheckedText -Program $Git -Arguments @("status", "--porcelain=v1", "--untracked-files=normal") -Label "git status"
        if ($GitStatus) {
            throw "publication builds require a clean Git working tree. Commit or remove these changes:`n$GitStatus"
        }

        $GitCommit = Invoke-CheckedText -Program $Git -Arguments @("rev-parse", "HEAD") -Label "git commit lookup"
        $GitShortCommit = Invoke-CheckedText -Program $Git -Arguments @("rev-parse", "--short=12", "HEAD") -Label "short git commit lookup"
        $GitBranch = Invoke-CheckedText -Program $Git -Arguments @("branch", "--show-current") -Label "git branch lookup"
        if (-not $GitBranch) {
            throw "publication builds require a named branch; detached HEAD is not allowed"
        }

        $GitUpstream = Invoke-CheckedText -Program $Git -Arguments @("rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}") -Label "git upstream lookup"
        $GitUpstreamCommit = Invoke-CheckedText -Program $Git -Arguments @("rev-parse", "@{u}") -Label "git upstream commit lookup"
        if ($GitUpstreamCommit -ne $GitCommit) {
            throw "local HEAD $GitCommit is not the pushed upstream commit $GitUpstreamCommit"
        }

        $GitRemote = Invoke-CheckedText -Program $Git -Arguments @("remote", "get-url", "origin") -Label "origin URL lookup"
        $RemoteHeadLine = Invoke-CheckedText -Program $Git -Arguments @("ls-remote", "--exit-code", "origin", "refs/heads/$GitBranch") -Label "remote branch lookup"
        $GitRemoteCommit = ([regex]::Split($RemoteHeadLine.Trim(), "\s+") | Select-Object -First 1)
        if ($GitRemoteCommit -ne $GitCommit) {
            throw "origin/$GitBranch is $GitRemoteCommit, but local HEAD is $GitCommit"
        }

        $GitTagsText = Invoke-CheckedText -Program $Git -Arguments @("tag", "--points-at", "HEAD") -Label "git tag lookup"
        $GitTags = @($GitTagsText -split "`r?`n" | Where-Object { $_ }) -join ", "
        if (-not $GitTags) { $GitTags = "(none)" }
    } finally {
        Pop-Location
    }


    $BuildUtc = [DateTime]::UtcNow
    $BuildUtcText = $BuildUtc.ToString(
        "yyyy-MM-dd'T'HH:mm:ss'Z'",
        [Globalization.CultureInfo]::InvariantCulture
    )
    $BuildStamp = $BuildUtc.ToString(
        "yyyyMMdd'T'HHmmss'Z'",
        [Globalization.CultureInfo]::InvariantCulture
    )
    $BuildId = "q0idmation-$ArtifactVersion-windows-x64-$GitShortCommit-$BuildStamp"

    Write-Host "[ok] beta version $BetaVersion" -ForegroundColor DarkGreen
    Write-Host "[ok] Rust host $RustHostTriple" -ForegroundColor DarkGreen
    Write-Host "[ok] Git commit $GitCommit ($GitBranch -> $GitUpstream)" -ForegroundColor DarkGreen

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

        & $CargoFmt fmt --all -- --check
        if ($LASTEXITCODE -ne 0) {
            throw "cargo fmt check exited with $LASTEXITCODE"
        }

        & $Cargo check --workspace --locked --target-dir $releaseTargetDir
        if ($LASTEXITCODE -ne 0) {
            throw "cargo check exited with $LASTEXITCODE"
        }

        & $Cargo test --workspace --locked --target-dir $releaseTargetDir
        if ($LASTEXITCODE -ne 0) {
            throw "cargo test exited with $LASTEXITCODE"
        }

        & $CargoClippy clippy --workspace --all-targets --locked --target-dir $releaseTargetDir -- -D warnings
        if ($LASTEXITCODE -ne 0) {
            throw "cargo clippy exited with $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }

    # -------- 2. Assets --------

    Step "Generating installer assets (icons + BMP)"
    & $Python "$InstallerDir\build_assets.py"
    if ($LASTEXITCODE -ne 0) {
        throw "build_assets.py exited with $LASTEXITCODE"
    }

    $GeneratedStatus = Invoke-CheckedText -Program $Git -Arguments @("status", "--porcelain=v1", "--untracked-files=normal") -Label "post-asset git status"
    if ($GeneratedStatus) {
        throw "installer asset generation changed the committed source tree. Commit the generated assets before publishing:`n$GeneratedStatus"
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
        & $Cargo build --release --locked -p q0editor -p q0player --target-dir $releaseTargetDir
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
            & $MakeNsis /V2 `
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
        "q0idmation beta build",
        "Build ID: $BuildId",
        "Version: $BetaVersion",
        "Build UTC: $BuildUtcText",
        "Git commit: $GitCommit",
        "Git branch: $GitBranch",
        "Git upstream: $GitUpstream",
        "Git tags: $GitTags",
        "Git remote: $GitRemote",
        "Git working tree: clean",
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
