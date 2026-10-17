# Octomind Installation Script for Windows
# Works with: Windows PowerShell 5.1, PowerShell 7+
#
# Usage:
#   irm https://raw.githubusercontent.com/Muvon/octomind/master/install.ps1 | iex
#
# Environment variables:
#   OCTOMIND_VERSION       Install a specific release tag (default: latest)
#   OCTOMIND_INSTALL_DIR   Installation directory (default: %LOCALAPPDATA%\octomind\bin)
#   GITHUB_TOKEN, GH_TOKEN Authenticate GitHub API requests (avoids rate limits in CI)

# Runs in its own script block so `irm | iex` does not leak variables or
# $ErrorActionPreference into the caller's session.
& {
	$ErrorActionPreference = 'Stop'
	# Windows PowerShell 5.1 renders the download progress bar slowly enough to
	# multiply download time.
	$ProgressPreference = 'SilentlyContinue'

	$Repo = 'Muvon/octomind'
	$BinaryName = 'octomind.exe'

	function Write-Info($Message) { Write-Host "[INFO] $Message" -ForegroundColor Cyan }
	function Write-Success($Message) { Write-Host "[SUCCESS] $Message" -ForegroundColor Green }
	function Write-Warn($Message) { Write-Host "[WARNING] $Message" -ForegroundColor Yellow }

	# $IsWindows only exists in PowerShell 6+; Windows PowerShell 5.1 leaves it unset.
	if ($IsWindows -eq $false) {
		throw 'install.ps1 installs the Windows build. On Linux and macOS use install.sh instead.'
	}

	# Windows PowerShell 5.1 may default to TLS 1.0, which GitHub rejects.
	if ($PSVersionTable.PSVersion.Major -lt 6) {
		[Net.ServicePointManager]::SecurityProtocol =
			[Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
	}

	function Get-Target {
		# A 32-bit PowerShell on 64-bit Windows reports x86 in PROCESSOR_ARCHITECTURE;
		# the real architecture is in PROCESSOR_ARCHITEW6432.
		$arch = $env:PROCESSOR_ARCHITEW6432
		if (-not $arch) { $arch = $env:PROCESSOR_ARCHITECTURE }
		# x64 PowerShell under emulation on ARM64 reports AMD64 in both variables;
		# the OS architecture tells the truth where the runtime exposes it.
		if ($arch -eq 'AMD64') {
			try {
				$osArch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
				if ($osArch -eq 'Arm64') { $arch = 'ARM64' }
			} catch {
				# Older .NET Framework without RuntimeInformation: keep the env value.
			}
		}
		switch ($arch) {
			'AMD64' { return 'x86_64-pc-windows-msvc' }
			'ARM64' { return 'aarch64-pc-windows-msvc' }
			default { throw "Unsupported architecture: $arch. Octomind ships Windows builds for x64 and ARM64 only." }
		}
	}

	function Get-Release($Version) {
		$headers = @{ 'User-Agent' = 'octomind-install'; 'Accept' = 'application/vnd.github+json' }
		$token = if ($env:GITHUB_TOKEN) { $env:GITHUB_TOKEN } else { $env:GH_TOKEN }
		if ($token) { $headers['Authorization'] = "token $token" }

		$url = "https://api.github.com/repos/$Repo/releases/latest"
		if ($Version) { $url = "https://api.github.com/repos/$Repo/releases/tags/$Version" }
		try {
			return Invoke-RestMethod -Uri $url -Headers $headers -UseBasicParsing
		} catch {
			$what = if ($Version) { "release $Version" } else { 'the latest release' }
			throw ("Failed to fetch $what from $url. Check the version and your connection, or set GITHUB_TOKEN " +
				"if you hit the API rate limit. ($($_.Exception.Message))")
		}
	}

	# Returns the expected SHA256 for $Asset: from the release's checksum file
	# when it lists the asset, else from the digest GitHub records for every
	# uploaded asset, else $null.
	function Get-ExpectedHash($Release, $Asset, $TmpDir) {
		$assetName = $Asset.name
		$checksumAsset = $Release.assets |
			Where-Object { $_.name -eq "$assetName.sha256" -or $_.name -match '^(SHA256SUMS|checksums\.txt)$' } |
			Sort-Object { $_.name -ne "$assetName.sha256" } |
			Select-Object -First 1
		if ($checksumAsset) {
			$checksumPath = Join-Path $TmpDir $checksumAsset.name
			try {
				Invoke-WebRequest -Uri $checksumAsset.browser_download_url -OutFile $checksumPath -UseBasicParsing
			} catch {
				throw "Download failed: $($checksumAsset.browser_download_url) ($($_.Exception.Message))"
			}
			# Accepts a bare hash or `sha256sum` lines (`<hash>  [*][dir/]<file>`).
			foreach ($line in Get-Content $checksumPath) {
				$fields = $line.Trim() -split '\s+', 2
				if ($fields[0] -notmatch '^[0-9a-fA-F]{64}$') { continue }
				if ($fields.Count -eq 1 -or (Split-Path -Leaf $fields[1].TrimStart('*')) -eq $assetName) {
					return $fields[0].ToLower()
				}
			}
		}
		if ($Asset.digest -match '^sha256:([0-9a-fA-F]{64})$') { return $Matches[1].ToLower() }
		if ($checksumAsset) { throw "Checksum file $($checksumAsset.name) has no SHA256 entry for $assetName" }
		return $null
	}

	# Edits the raw registry value so entries like %USERPROFILE%\bin stay
	# unexpanded; [Environment]::SetEnvironmentVariable would rewrite them as REG_SZ.
	function Add-ToUserPath($Dir) {
		$key = Get-Item -Path 'HKCU:\Environment'
		$current = $key.GetValue('Path', '', 'DoNotExpandEnvironmentNames')
		$entries = @($current -split ';' | Where-Object { $_ })
		$normalized = $Dir.TrimEnd('\')
		$present = $entries |
			Where-Object { [Environment]::ExpandEnvironmentVariables($_).TrimEnd('\') -eq $normalized }
		if ($present) { return $false }

		$updated = (@($entries) + $Dir) -join ';'
		Set-ItemProperty -Path 'HKCU:\Environment' -Name 'Path' -Value $updated -Type ExpandString

		# Tell Explorer and other top-level windows that the environment changed, so
		# terminals opened from them see the new PATH without signing out. Best
		# effort: Add-Type is unavailable under Constrained Language Mode, and the
		# PATH is already written either way.
		try {
			if (-not ('Octomind.NativeMethods' -as [type])) {
				Add-Type -Namespace Octomind -Name NativeMethods -MemberDefinition @'
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(
	IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
'@
			}
			$HWND_BROADCAST = [IntPtr]0xffff
			$WM_SETTINGCHANGE = 0x1a
			$SMTO_ABORTIFHUNG = 0x2
			$result = [UIntPtr]::Zero
			[void][Octomind.NativeMethods]::SendMessageTimeout(
				$HWND_BROADCAST, $WM_SETTINGCHANGE, [UIntPtr]::Zero, 'Environment',
				$SMTO_ABORTIFHUNG, 5000, [ref]$result)
		} catch {
			Write-Warn 'Could not broadcast the PATH change; sign out and back in if octomind is not found'
		}
		return $true
	}

	$installDir = Join-Path $env:LOCALAPPDATA 'octomind\bin'
	if ($env:OCTOMIND_INSTALL_DIR) {
		# A relative entry in the user PATH would resolve against every shell's
		# current directory, so store the absolute path.
		$installDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($env:OCTOMIND_INSTALL_DIR)
	}

	$target = Get-Target
	Write-Info "Detected platform: $target"

	if ($env:OCTOMIND_VERSION) {
		Write-Info "Fetching release $env:OCTOMIND_VERSION..."
	} else {
		Write-Info 'Fetching latest release information...'
	}
	$release = Get-Release $env:OCTOMIND_VERSION
	$version = $release.tag_name

	$assetName = "octomind-$version-$target.zip"
	$asset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
	if (-not $asset) { throw "Release $version has no asset named $assetName" }

	$tmpDir = Join-Path ([IO.Path]::GetTempPath()) ("octomind-install-" + [Guid]::NewGuid().ToString('N'))
	New-Item -ItemType Directory -Path $tmpDir | Out-Null
	try {
		$zipPath = Join-Path $tmpDir $assetName
		Write-Info "Downloading from: $($asset.browser_download_url)"
		try {
			Invoke-WebRequest -Uri $asset.browser_download_url -OutFile $zipPath -UseBasicParsing
		} catch {
			throw "Download failed: $($asset.browser_download_url) ($($_.Exception.Message))"
		}

		$expected = Get-ExpectedHash $release $asset $tmpDir
		if ($expected) {
			$actual = (Get-FileHash -Path $zipPath -Algorithm SHA256).Hash.ToLower()
			if ($actual -ne $expected) { throw "Checksum mismatch for ${assetName}: expected $expected, got $actual" }
			Write-Info 'Checksum verified'
		} else {
			Write-Warn "No checksum available for $assetName; skipping verification"
		}

		Write-Info 'Extracting binary...'
		$extractDir = Join-Path $tmpDir 'extract'
		Expand-Archive -Path $zipPath -DestinationPath $extractDir -Force
		$binaryPath = Join-Path $extractDir $BinaryName
		if (-not (Test-Path $binaryPath)) { throw "Binary $BinaryName not found in $assetName" }

		Write-Info "Installing to $installDir..."
		New-Item -ItemType Directory -Path $installDir -Force | Out-Null
		$targetPath = Join-Path $installDir $BinaryName
		try {
			Copy-Item -Path $binaryPath -Destination $targetPath -Force
		} catch {
			throw "Failed to write $targetPath. Close any running octomind and try again. ($($_.Exception.Message))"
		}
	} finally {
		Remove-Item -Path $tmpDir -Recurse -Force -ErrorAction SilentlyContinue
	}

	if (Add-ToUserPath $installDir) {
		Write-Info "Added $installDir to your user PATH"
	}
	if (-not (($env:Path -split ';') | Where-Object { $_.TrimEnd('\') -eq $installDir.TrimEnd('\') })) {
		$env:Path = "$installDir;$env:Path"
	}

	# The release binary links the dynamic MSVC runtime, so a missing Visual C++
	# Redistributable surfaces here instead of on first use.
	try {
		$installed = & $targetPath --version
	} catch {
		$installed = $null
	}
	if ($LASTEXITCODE -ne 0 -or -not $installed) {
		throw ("Installed $targetPath but it failed to run. Install the Microsoft Visual C++ Redistributable " +
			'(https://aka.ms/vs/17/release/vc_redist.x64.exe for x64, ' +
			'https://aka.ms/vs/17/release/vc_redist.arm64.exe for ARM64) and try again.')
	}

	Write-Success "Installed $installed to $targetPath"
	Write-Info 'Open a new terminal if octomind is not found, then run: octomind --help'
}
