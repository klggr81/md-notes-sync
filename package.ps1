# Builds the release zip: md-notes-sync.exe with the README and SETUP.md (and their screenshots
# in docs\), the changelog, the license and the third-party licenses, ready to
# unpack and start. Used by the GitHub build (.github/workflows/build.yml) and locally.
#
#   package.ps1            build the exe, then zip it
#   package.ps1 -NoBuild   zip an exe that is already built
#
# Output: release\md-notes-sync-<version>.zip
param([switch]$NoBuild)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot

# One version for everything: [workspace.package] in Cargo.toml
$version = (Select-String -Path (Join-Path $root 'Cargo.toml') -Pattern '^version\s*=\s*"([^"]+)"').Matches[0].Groups[1].Value
if (-not $version) { throw 'No version found in Cargo.toml' }

if (-not $NoBuild) {
  Push-Location $root
  # npm and cargo write progress to stderr, which PowerShell 5.1 would treat as
  # errors: only the exit code counts
  $ErrorActionPreference = 'Continue'
  try {
    npm.cmd run tauri build -- --no-bundle
    if ($LASTEXITCODE) { throw "tauri build failed (exit $LASTEXITCODE)" }
  } finally {
    Pop-Location
    $ErrorActionPreference = 'Stop'
  }
}
$exe = Join-Path $root 'target\release\md-notes-sync.exe'
if (-not (Test-Path $exe)) { throw "Not built yet: $exe" }

$name = "md-notes-sync-$version"
$out = Join-Path $root 'release'
$stage = Join-Path $out $name
$zip = "$stage.zip"
if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
if (Test-Path $zip) { Remove-Item $zip -Force }
New-Item -ItemType Directory -Path $stage | Out-Null

Copy-Item $exe $stage
foreach ($f in 'README.md', 'SETUP.md', 'CHANGELOG.md', 'LICENSE') { Copy-Item (Join-Path $root $f) $stage }
# The screenshots in the README and SETUP.md
Copy-Item (Join-Path $root 'docs') $stage -Recurse

# The licenses of the Rust crates in the exe (about.toml, about.hbs)
if (-not (Get-Command cargo-about -ErrorAction SilentlyContinue)) {
  throw 'cargo-about is missing: cargo install cargo-about --locked --features cli'
}
Push-Location $root
$ErrorActionPreference = 'Continue'
try {
  cargo about generate about.hbs -o (Join-Path $stage 'THIRD-PARTY-NOTICES.html')
  if ($LASTEXITCODE) { throw "cargo about failed (exit $LASTEXITCODE)" }
} finally {
  Pop-Location
  $ErrorActionPreference = 'Stop'
}

Add-Type -AssemblyName System.IO.Compression.FileSystem
# includeBaseDirectory: the zip unpacks into one folder, md-notes-sync-<version>\
[IO.Compression.ZipFile]::CreateFromDirectory($stage, $zip, [IO.Compression.CompressionLevel]::Optimal, $true)

$mb = [math]::Round((Get-Item $exe).Length / 1MB, 1)
Write-Host "Built $zip (exe $mb MB)" -ForegroundColor Green
