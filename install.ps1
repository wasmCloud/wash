# Install script for wash - The Wasm Shell (Windows PowerShell)
# Usage: iwr -useb https://raw.githubusercontent.com/wasmcloud/wash/main/install.ps1 | iex
# Usage with options: ./install.ps1 -InstallDir "C:\tools" -AddToPath -Force
#
# Parameters:
# - InstallDir: Directory to install wash binary (default: current directory)
# - Version: Install a specific version (e.g., "1.0.0-beta.9", "v1.0.0-beta.9", or "wash-v1.0.0-beta.10")
# - AddToPath: Automatically add install directory to user PATH
# - Force: Overwrite existing installation without prompting
#
# Environment variables:
# - $env:GITHUB_TOKEN: GitHub personal access token (optional, for higher API rate limits)
# - $env:INSTALL_DIR: Directory to install wash binary (overrides -InstallDir)

param(
    [string]$InstallDir = $(if ($env:INSTALL_DIR) { $env:INSTALL_DIR } else { $PWD }),
    [string]$GitHubToken = $env:GITHUB_TOKEN,
    [string]$Version = "",
    [switch]$AddToPath,
    [switch]$Force
)

# Set strict mode
Set-StrictMode -Version 3.0
$ErrorActionPreference = 'Stop'

# Constants
$REPO = "wasmcloud/wash"
$TMP_DIR = Join-Path $env:TEMP "wash-install-$((Get-Date).Ticks)"

# Helper functions
function Write-Info {
    param([string]$Message)
    Write-Host "[INFO] $Message" -ForegroundColor Blue
}

function Write-Success {
    param([string]$Message)
    Write-Host "[SUCCESS] $Message" -ForegroundColor Green
}

function Write-Warn {
    param([string]$Message)
    Write-Host "[WARN] $Message" -ForegroundColor Yellow
}

function Write-Error {
    param([string]$Message)
    Write-Host "[ERROR] $Message" -ForegroundColor Red
}

function Cleanup {
    if (Test-Path $TMP_DIR) {
        Remove-Item -Recurse -Force $TMP_DIR -ErrorAction SilentlyContinue
    }
}

# Add directory to PATH
function Add-ToPath {
    param([string]$Directory)
    
    $currentPath = [Environment]::GetEnvironmentVariable("PATH", "User")
    
    # Check if directory is already in PATH
    if ($currentPath -split ';' | Where-Object { $_ -eq $Directory }) {
        Write-Info "Directory $Directory is already in PATH"
        return
    }
    
    try {
        $newPath = if ($currentPath) { "$currentPath;$Directory" } else { $Directory }
        [Environment]::SetEnvironmentVariable("PATH", $newPath, "User")
        Write-Success "Added $Directory to user PATH"
        Write-Info "Please restart your terminal or run: refreshenv"
    }
    catch {
        Write-Error "Failed to add $Directory to PATH: $($_.Exception.Message)"
        Write-Info "You can manually add it using:"
        Write-Host "  [Environment]::SetEnvironmentVariable('PATH', `$env:PATH + ';$Directory', 'User')"
    }
}

# Test if running in Windows Terminal, PowerShell ISE, or regular console
function Test-InteractiveSession {
    return $Host.Name -match "ConsoleHost|ISE"
}

# Cleanup on exit
Register-EngineEvent -SourceIdentifier PowerShell.Exiting -Action { Cleanup }

# Detect platform
function Get-Platform {
    $arch = $env:PROCESSOR_ARCHITECTURE
    
    switch ($arch) {
        "AMD64" { return "x86_64-pc-windows-msvc" }
        default { 
            Write-Error "Unsupported architecture: $arch"
            exit 1
        }
    }
}

# Get latest release information from GitHub API
function Get-LatestRelease {
    $apiUrl = "https://api.github.com/repos/$REPO/releases/latest"
    $headers = @{
        'User-Agent' = 'wash-installer'
    }

    if ($GitHubToken) {
        $headers['Authorization'] = "token $GitHubToken"
        Write-Info "Using GitHub token for API access"
    }

    Write-Info "Fetching latest release information..."

    try {
        $response = Invoke-RestMethod -Uri $apiUrl -Headers $headers -ErrorAction Stop
    }
    catch {
        if ($_.Exception.Response.StatusCode -eq 404) {
            Write-Error "Repository $REPO not found or has no releases"
            Write-Error "Please verify the repository exists and has published releases"
        }
        else {
            Write-Error "Failed to fetch release information from GitHub API"
            Write-Error "Please check your internet connection and try again"
            Write-Error "Error: $($_.Exception.Message)"
        }
        exit 1
    }

    if (-not $response.tag_name) {
        Write-Error "No releases found for repository $REPO"
        Write-Error "Please verify the repository has published releases"
        exit 1
    }

    return $response.tag_name
}

# Get release information for a specific version
function Get-ReleaseByVersion {
    param([string]$RequestedVersion)

    # Normalize version format - wash releases use 'wash-v' prefix
    if (-not $RequestedVersion.StartsWith('wash-v')) {
        # Remove any leading 'v' if present
        if ($RequestedVersion.StartsWith('v')) {
            $RequestedVersion = $RequestedVersion.Substring(1)
        }
        # Add 'wash-v' prefix
        $RequestedVersion = "wash-v$RequestedVersion"
    }

    $apiUrl = "https://api.github.com/repos/$REPO/releases/tags/$RequestedVersion"
    $headers = @{
        'User-Agent' = 'wash-installer'
    }

    if ($GitHubToken) {
        $headers['Authorization'] = "token $GitHubToken"
    }

    Write-Info "Fetching release information for version $RequestedVersion..."

    try {
        $response = Invoke-RestMethod -Uri $apiUrl -Headers $headers -ErrorAction Stop
    }
    catch {
        if ($_.Exception.Response.StatusCode -eq 404) {
            Write-Error "Version $RequestedVersion not found"
            Write-Error "Please verify the version exists. You can check available versions at:"
            Write-Error "https://github.com/$REPO/releases"
        }
        else {
            Write-Error "Failed to fetch release information from GitHub API"
            Write-Error "Please check your internet connection and try again"
            Write-Error "Error: $($_.Exception.Message)"
        }
        exit 1
    }

    if (-not $response.tag_name) {
        Write-Error "Version $RequestedVersion not found"
        exit 1
    }

    return $response.tag_name
}

# Get asset ID for the specified platform
function Get-AssetIdForPlatform {
    param(
        [string]$Platform,
        [string]$TargetVersion
    )

    $expectedName = "wash-$Platform"

    if ($TargetVersion) {
        $apiUrl = "https://api.github.com/repos/$REPO/releases/tags/$TargetVersion"
    } else {
        $apiUrl = "https://api.github.com/repos/$REPO/releases/latest"
    }

    $headers = @{
        'User-Agent' = 'wash-installer'
    }

    if ($GitHubToken) {
        $headers['Authorization'] = "token $GitHubToken"
    }

    try {
        $response = Invoke-RestMethod -Uri $apiUrl -Headers $headers -ErrorAction Stop
    }
    catch {
        Write-Error "Failed to fetch release information for asset lookup"
        return $null
    }
    
    $asset = $response.assets | Where-Object { $_.name -eq $expectedName }
    
    if ($asset) {
        return $asset.id
    }
    else {
        return $null
    }
}

# Download and install wash binary
function Install-Wash {
    param(
        [string]$Platform,
        [string]$TargetVersion
    )

    $binaryName = "wash-$Platform"

    Write-Info "Detected platform: $Platform"
    Write-Info "Version: $TargetVersion"

    # Get the asset ID for our platform
    Write-Info "Finding asset for platform..."
    $assetId = Get-AssetIdForPlatform $Platform $TargetVersion
    
    if (-not $assetId) {
        Write-Error "No matching binary found for platform $Platform"
        Write-Error "Available assets:"
        
        # Show available assets
        $apiUrl = "https://api.github.com/repos/$REPO/releases/latest"
        $headers = @{ 'User-Agent' = 'wash-installer' }
        if ($GitHubToken) {
            $headers['Authorization'] = "token $GitHubToken"
        }
        
        try {
            $response = Invoke-RestMethod -Uri $apiUrl -Headers $headers
            $response.assets | ForEach-Object { Write-Host "  - $($_.name)" }
        }
        catch {
            Write-Error "Could not fetch available assets"
        }
        exit 1
    }
    
    $downloadUrl = "https://api.github.com/repos/$REPO/releases/assets/$assetId"
    Write-Info "Download URL: $downloadUrl"
    
    # Create temporary directory
    New-Item -ItemType Directory -Path $TMP_DIR -Force | Out-Null
    
    # Download binary using GitHub API
    Write-Info "Downloading wash binary..."
    $headers = @{
        'Accept' = 'application/octet-stream'
        'User-Agent' = 'wash-installer'
    }
    
    if ($GitHubToken) {
        $headers['Authorization'] = "token $GitHubToken"
    }
    
    $downloadPath = Join-Path $TMP_DIR "wash.exe"
    
    try {
        Invoke-WebRequest -Uri $downloadUrl -Headers $headers -OutFile $downloadPath -ErrorAction Stop
        Write-Success "Download completed successfully"
    }
    catch {
        Write-Error "Failed to download wash binary from $downloadUrl"
        Write-Error "Error: $($_.Exception.Message)"
        exit 1
    }
    
    # Create install directory if it doesn't exist
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }
    
    # Move binary to install directory
    $installPath = Join-Path $InstallDir "wash.exe"
    
    # Check if file already exists
    if ((Test-Path $installPath) -and -not $Force) {
        Write-Warn "wash.exe already exists at $installPath"
        if (Test-InteractiveSession) {
            $response = Read-Host "Overwrite existing installation? (y/N)"
            if ($response -notmatch '^[Yy]') {
                Write-Info "Installation cancelled by user"
                exit 0
            }
        } else {
            Write-Error "Existing installation found. Use -Force flag to overwrite automatically"
            exit 1
        }
    }
    
    try {
        Move-Item -Path $downloadPath -Destination $installPath -Force
    }
    catch {
        Write-Error "Failed to install wash to $InstallDir"
        Write-Error "Error: $($_.Exception.Message)"
        exit 1
    }
    
    Write-Success "wash $TargetVersion installed successfully to $installPath"
    
    # Test installation
    try {
        $testOutput = & $installPath --help 2>$null
        if ($LASTEXITCODE -eq 0) {
            Write-Success "Verified installation: wash --help works correctly"
        }
        else {
            Write-Warn "Could not verify installation. Try running: $installPath --help"
        }
    }
    catch {
        Write-Warn "Could not verify installation. Try running: $installPath --help"
    }
    
    # Show next steps
    Write-Host ""
    Write-Info "Next steps:"
    Write-Host "  1. Add $InstallDir to your PATH if not already included"
    Write-Host "  2. Run 'wash --help' to see available commands"
    Write-Host "  3. Run 'wash doctor' to verify your environment"
    Write-Host "  4. Run 'wash new' to create your first WebAssembly component"
    Write-Host ""
    
    # Handle PATH addition
    if ($AddToPath) {
        Add-ToPath $InstallDir
    } else {
        Write-Info "To add to PATH for current session:"
        Write-Host "  `$env:PATH += ';$InstallDir'"
        Write-Host ""
        Write-Info "To add to PATH permanently:"
        Write-Host "  [Environment]::SetEnvironmentVariable('PATH', `$env:PATH + ';$InstallDir', 'User')"
        Write-Host ""
        Write-Info "Or run the installer again with -AddToPath flag"
    }
}

# Main execution
function Main {
    Write-Info "Installing wash - The Wasm Shell"
    Write-Host ""
    
    # Check for GitHub token (optional, for higher API rate limits)
    if (-not $GitHubToken) {
        Write-Info "No GitHub token provided. Using anonymous API access (subject to rate limits)"
        Write-Info "To avoid rate limits, set GITHUB_TOKEN environment variable"
    } else {
        Write-Info "Using GitHub token for API access"
    }
    
    # Check PowerShell version
    if ($PSVersionTable.PSVersion.Major -lt 5) {
        Write-Error "PowerShell 5.0 or higher is required"
        Write-Error "Current version: $($PSVersionTable.PSVersion)"
        exit 1
    }
    Write-Info "PowerShell version check passed"
    
    # Check if running as administrator (optional warning)
    $isAdmin = ([Security.Principal.WindowsPrincipal] [Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole] "Administrator")
    if ($isAdmin) {
        Write-Warn "Running as administrator. Consider running as a regular user for security."
    }
    
    # Detect platform
    Write-Info "Detecting platform..."
    $platform = Get-Platform
    Write-Info "Platform detected: $platform"
    
    # Get release version
    if ($Version) {
        Write-Info "Fetching release information for version $Version..."
        $targetVersion = Get-ReleaseByVersion $Version
    } else {
        Write-Info "Fetching latest release information..."
        $targetVersion = Get-LatestRelease
    }
    Write-Info "Version: $targetVersion"

    # Install wash
    Install-Wash -Platform $platform -TargetVersion $targetVersion
}

# Run main function
try {
    Main
}
catch {
    Write-Error "Installation failed: $($_.Exception.Message)"
    exit 1
}
finally {
    Cleanup
}
