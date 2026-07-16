<#
.SYNOPSIS
Starts the repository's manual GitHub Actions source build.

.EXAMPLE
.\scripts\trigger-github-build.ps1

.EXAMPLE
.\scripts\trigger-github-build.ps1 -Ref feature/workflows
#>
[CmdletBinding()]
param(
    [string]$Repo = "EwanJordaan/codex-workflow",
    [string]$Ref = "main"
)

$ErrorActionPreference = "Stop"

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    throw "GitHub CLI (gh) is required. Install it from https://cli.github.com/."
}

gh workflow run build-source.yml --repo $Repo --ref $Ref --field "ref=$Ref"
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

Write-Host "Build started. View it with: gh run list --repo $Repo --workflow build-source.yml"
