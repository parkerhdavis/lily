# ─── Lily Makefile ───

.PHONY: help dev dev-frontend down build build-linux build-windows build-macos notarize setup install lint lint-fix format check test typecheck clean version corpus-update corpus-check

# ─── OS Detection ───
# On Windows, uname doesn't exist, so we check for Windows-specific env vars first
ifdef OS
    # Windows sets OS=Windows_NT
    ifeq ($(OS),Windows_NT)
        UNAME_S := Windows
    else
        UNAME_S := $(shell uname -s 2>/dev/null || echo Windows)
    endif
else
    UNAME_S := $(shell uname -s 2>/dev/null || echo Windows)
endif
ifneq (,$(findstring MINGW,$(UNAME_S)))
    DETECTED_OS := windows
else ifneq (,$(findstring MSYS,$(UNAME_S)))
    DETECTED_OS := windows
else ifneq (,$(findstring CYGWIN,$(UNAME_S)))
    DETECTED_OS := windows
else ifneq (,$(findstring Windows,$(UNAME_S)))
    DETECTED_OS := windows
else ifeq ($(UNAME_S),Linux)
    DETECTED_OS := linux
else ifeq ($(UNAME_S),Darwin)
    DETECTED_OS := macos
else
    DETECTED_OS := windows
endif

# ─── Paths ───
# Apps live under apps/<name>/ per the portfolio structure. Lily is a
# single-deployment Tauri desktop app, so one app dir: apps/desktop/.
BACKEND := apps/desktop/backend
FRONTEND := apps/desktop/frontend

# Windows-specific: use PowerShell 7 (pwsh) for complex commands
ifeq ($(DETECTED_OS),windows)
    SHELL := pwsh.exe
    .SHELLFLAGS := -NoProfile -Command
    BUN := bun
    # @tauri-apps/cli is a root devDependency, so Bun hoists the bin to
    # <repo>/node_modules/.bin/tauri and bunx resolves from any cwd.
    TAURI := bunx tauri
    MKDIR := New-Item -ItemType Directory -Force -Path
    RM := Remove-Item -Recurse -Force -ErrorAction SilentlyContinue
    NULL := $$null
else
    BUN := bun
    # @tauri-apps/cli is a root devDependency, so Bun hoists the bin to
    # <repo>/node_modules/.bin/tauri and bunx resolves from any cwd.
    TAURI := bunx tauri
    MKDIR := mkdir -p
    RM := rm -rf
    NULL := /dev/null
    ifeq ($(DETECTED_OS),macos)
        SED_INPLACE := sed -i ''
    else
        SED_INPLACE := sed -i
    endif
endif

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort -u | awk 'BEGIN {FS = ":.*?## "}; {printf "\033[36m%-20s\033[0m %s\n", $$1, $$2}'

# ─── Run ───

ifeq ($(DETECTED_OS),windows)
dev: ## Start Tauri dev server (frontend + Rust hot-reload)
	@echo "Starting Tauri development server (frontend + Rust)..."
	cd $(BACKEND); $(TAURI) dev

dev-frontend: ## Start Bun dev server only (rapid UI iteration)
	@echo "Starting Bun dev server only (rapid UI iteration)..."
	cd $(FRONTEND); $(BUN) run dev

down: ## Stop any running dev server
	@echo "Stopping dev server..."
	@echo "On Windows, close the terminal running the dev server or use Task Manager."
else
dev: ## Start Tauri dev server (frontend + Rust hot-reload)
	@echo "Starting Tauri development server (frontend + Rust)..."
	@# Warn about and kill any leftover dev server on port 5173
	@EXISTING_PID=$$(lsof -ti :5173 2>/dev/null); \
	if [ -n "$$EXISTING_PID" ]; then \
		echo "  -> WARNING: Port 5173 in use (pid $$EXISTING_PID) — killing to free port"; \
		kill $$EXISTING_PID 2>/dev/null || true; \
		sleep 1; \
	fi
	@echo "  -> Starting Tauri (frontend dev server started by Tauri via beforeDevCommand)..."
	@cd $(BACKEND) && $(TAURI) dev

down: ## Stop any running dev server
	@echo "Stopping Lily dev server..."
	@echo "  -> Checking port 5173..."
	@PORT_PID=$$(lsof -ti :5173 2>/dev/null); \
	if [ -n "$$PORT_PID" ]; then \
		kill $$PORT_PID 2>/dev/null || true; \
		echo "  -> Killed process on port 5173 (pid $$PORT_PID)"; \
	else \
		echo "  -> No dev server running"; \
	fi

dev-frontend: ## Start Bun dev server only (rapid UI iteration)
	@echo "Starting Bun dev server only (rapid UI iteration)..."
	@cd $(FRONTEND) && $(BUN) run dev
endif

# ─── Setup ───
# Probes for Rust + Bun + system deps (webkit2gtk on Linux, Xcode CLT on
# macOS, VS Build Tools on Windows), then runs `bun install` at the root.

ifeq ($(DETECTED_OS),windows)
setup: ## Check/install Rust + Bun + system deps, then bun install
	@echo "================================================================================"
	@echo "  Lily Setup - Installing Dependencies"
	@echo "================================================================================"
	@if (-not (Get-Command rustc -ErrorAction SilentlyContinue)) { Write-Host "Rust not found. Install from https://rustup.rs then re-run 'make setup'."; exit 1 } else { Write-Host "Rust: $$(rustc --version)" }
	@if (-not (Get-Command bun -ErrorAction SilentlyContinue)) { Write-Host "Bun not found. Install from https://bun.sh then re-run 'make setup'."; exit 1 } else { Write-Host "Bun: $$(bun --version)" }
	@Write-Host ""
	@Write-Host "Windows system requirements for Tauri:"
	@Write-Host "  - Visual Studio C++ Build Tools"
	@Write-Host "  - WebView2 Runtime (pre-installed on Windows 10+)"
	@Write-Host "  See https://tauri.app/start/prerequisites/#windows"
	@Write-Host ""
	@Write-Host "Installing JS dependencies..."
	$(BUN) install
	@Write-Host "Setup complete"

install: setup
else
setup: ## Check/install Rust + Bun + system deps, then bun install
	@echo "================================================================================"
	@echo "  Lily Setup - Installing Dependencies"
	@echo "================================================================================"
	@if ! command -v rustc >/dev/null 2>&1; then \
		echo "Rust not found. Installing via rustup..."; \
		curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh; \
		echo "Rust installed. Please restart your shell and re-run 'make setup'."; \
		exit 0; \
	else \
		echo "Rust: $$(rustc --version)"; \
	fi
	@if ! command -v bun >/dev/null 2>&1; then \
		echo "Bun not found. Installing..."; \
		curl -fsSL https://bun.sh/install | bash; \
		echo "Bun installed. Please restart your shell and re-run 'make setup'."; \
		exit 0; \
	else \
		echo "Bun: $$(bun --version)"; \
	fi
ifeq ($(DETECTED_OS),linux)
	@echo ""
	@echo "Checking Linux system dependencies for Tauri..."
	@MISSING=""; \
	for p in libwebkit2gtk-4.1-dev build-essential curl wget file libssl-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev; do \
		if ! dpkg-query -W -f='$${Status}' "$$p" 2>/dev/null | grep -q "install ok installed"; then \
			MISSING="$$MISSING $$p"; \
		fi; \
	done; \
	if [ -n "$$MISSING" ]; then \
		echo "Missing system packages:$$MISSING"; \
		echo ""; \
		echo "Install with: sudo apt install$$MISSING"; \
		echo ""; \
		echo "Continuing with JS install anyway — Tauri build will fail until these land."; \
	else \
		echo "All required system packages present."; \
	fi
else ifeq ($(DETECTED_OS),macos)
	@echo ""
	@echo "Checking macOS system dependencies..."
	@if ! xcode-select -p >/dev/null 2>&1; then \
		echo "Xcode Command Line Tools not installed."; \
		echo "Install with: xcode-select --install"; \
	else \
		echo "Xcode Command Line Tools installed."; \
	fi
endif
	@echo ""
	@echo "Installing JS dependencies..."
	@$(BUN) install
	@echo "Setup complete"

install: setup
endif

# ─── Build ───

ifeq ($(DETECTED_OS),windows)
build: ## Build for current platform (auto-detects OS)
	@echo "Building Windows installers (.msi, .exe)..."
	@echo "  -> Building frontend..."
	cd $(FRONTEND); $(BUN) run build
	@echo "  -> Building Tauri app for Windows..."
	$$env:PATH = "$$env:USERPROFILE\.cargo\bin;$$env:PATH"; cd $(BACKEND); $(TAURI) build
	@echo ""
	@echo "Windows build complete!"
	@echo ""
	@echo "Build outputs in ./target/release/bundle/:"
	@echo "  - MSI Installer:  ./target/release/bundle/msi/"
	@echo "  - NSIS Installer: ./target/release/bundle/nsis/"
else
build: ## Build for current platform (auto-detects OS)
ifeq ($(DETECTED_OS),linux)
	@$(MAKE) build-linux
else ifeq ($(DETECTED_OS),macos)
	@$(MAKE) build-macos
endif
endif

ifeq ($(DETECTED_OS),windows)
build-linux: ## Build Linux installers (.deb, .rpm, AppImage)
	@echo "ERROR: Linux builds must be run on Linux"
	@exit 1

build-windows: ## Build Windows installers (.msi, .exe)
	@echo "Building Windows installers (.msi, .exe)..."
	@echo "  -> Building frontend..."
	cd $(FRONTEND); $(BUN) run build
	@echo "  -> Building Tauri app for Windows..."
	$$env:PATH = "$$env:USERPROFILE\.cargo\bin;$$env:PATH"; cd $(BACKEND); $(TAURI) build
	@echo ""
	@echo "Windows build complete!"
	@echo ""
	@echo "Build outputs in ./target/release/bundle/:"
	@echo "  - MSI Installer:  ./target/release/bundle/msi/"
	@echo "  - NSIS Installer: ./target/release/bundle/nsis/"

build-macos: ## Build macOS installers (.dmg, .app)
	@echo "ERROR: macOS builds must be run on macOS"
	@exit 1

notarize: ## Submit macOS build for Apple notarization
	@echo "ERROR: macOS notarization must be run on macOS"
	@exit 1
else
build-linux: ## Build Linux installers (.deb, .rpm, AppImage)
	@echo "Building Linux installers (.deb, .rpm, AppImage)..."
	@echo "  -> Building frontend..."
	@cd $(FRONTEND) && $(BUN) run build
	@echo "  -> Building Tauri app for Linux..."
	@cd $(BACKEND) && $(TAURI) build
	@echo ""
	@echo "Linux build complete!"
	@echo ""
	@echo "Build outputs in ./target/release/bundle/:"
	@echo "  - AppImage: ./target/release/bundle/appimage/"
	@echo "  - Debian:   ./target/release/bundle/deb/"
	@echo "  - RPM:      ./target/release/bundle/rpm/"

build-windows: ## Build Windows installers (.msi, .exe)
	@echo "ERROR: Windows builds must be run on Windows"
	@exit 1

build-macos: ## Build macOS installers (.dmg, .app)
	@echo "Building macOS installers (.dmg, .app)..."
	@if [ -f .env ]; then \
		echo "  -> Loading signing config from .env"; \
	else \
		echo "  -> WARNING: No .env file found — build will not be signed"; \
		echo "     Copy .env.example to .env and fill in your Apple credentials"; \
	fi
	@echo "  -> Building frontend..."
	@cd $(FRONTEND) && $(BUN) run build
	@echo "  -> Building Tauri app for macOS..."
	@if [ -f .env ]; then \
		. ./.env && \
		( unset APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID && cd $(BACKEND) && $(TAURI) build ); \
	else \
		cd $(BACKEND) && $(TAURI) build; \
	fi
	@if [ -f .env ]; then \
		. ./.env && \
		echo "" && \
		echo "Re-signing with hardened runtime and secure timestamp..." && \
		echo "  (Tauri's bundler omits --timestamp; re-signing to fix)" && \
		codesign --force --options runtime --timestamp \
			--entitlements $(BACKEND)/entitlements.plist \
			--sign "$$APPLE_SIGNING_IDENTITY" \
			./target/release/bundle/macos/Lily.app/Contents/MacOS/lily && \
		codesign --force --options runtime --timestamp \
			--entitlements $(BACKEND)/entitlements.plist \
			--sign "$$APPLE_SIGNING_IDENTITY" \
			./target/release/bundle/macos/Lily.app && \
		echo "  -> Re-signed .app bundle" && \
		echo "" && \
		echo "Rebuilding DMG from re-signed .app..." && \
		VERSION=$$(grep '^version = ' $(BACKEND)/Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/') && \
		ARCH=$$(uname -m | sed 's/arm64/aarch64/') && \
		DMG_NAME="Lily_$${VERSION}_$${ARCH}.dmg" && \
		DMG_PATH="./target/release/bundle/dmg/$${DMG_NAME}" && \
		rm -f "$${DMG_PATH}" && \
		hdiutil create -volname "Lily" \
			-srcfolder ./target/release/bundle/macos/Lily.app \
			-ov -format UDZO \
			"$${DMG_PATH}" && \
		codesign --force --timestamp \
			--sign "$$APPLE_SIGNING_IDENTITY" \
			"$${DMG_PATH}" && \
		echo "  -> DMG rebuilt and signed: $${DMG_PATH}" && \
		echo "" && \
		echo "Verifying code signature..." && \
		codesign --verify --deep --strict ./target/release/bundle/macos/Lily.app && \
		echo "  -> Code signature: OK"; \
	fi
	@echo ""
	@echo "macOS build complete!"
	@echo ""
	@echo "Build outputs in ./target/release/bundle/:"
	@echo "  - DMG:  ./target/release/bundle/dmg/"
	@echo "  - App:  ./target/release/bundle/macos/"
	@if [ -f .env ]; then \
		echo "" && \
		echo "To notarize, run: make notarize"; \
	fi

notarize: ## Submit macOS build for Apple notarization
	@if [ ! -f .env ]; then \
		echo "ERROR: .env file required for notarization"; \
		echo "  Copy .env.example to .env and fill in your Apple credentials"; \
		exit 1; \
	fi
	@. ./.env && \
	VERSION=$$(grep '^version = ' $(BACKEND)/Cargo.toml | head -1 | sed 's/version = "\(.*\)"/\1/') && \
	ARCH=$$(uname -m | sed 's/arm64/aarch64/') && \
	DMG_NAME="Lily_$${VERSION}_$${ARCH}.dmg" && \
	DMG_PATH="./target/release/bundle/dmg/$${DMG_NAME}" && \
	if [ ! -f "$${DMG_PATH}" ]; then \
		echo "ERROR: DMG not found at $${DMG_PATH}"; \
		echo "  Run 'make build-macos' first"; \
		exit 1; \
	fi && \
	echo "Submitting $${DMG_NAME} for notarization..." && \
	xcrun notarytool submit "$${DMG_PATH}" \
		--apple-id "$$APPLE_ID" \
		--password "$$APPLE_PASSWORD" \
		--team-id "$$APPLE_TEAM_ID" \
		--wait && \
	echo "" && \
	echo "Stapling notarization ticket..." && \
	xcrun stapler staple "$${DMG_PATH}" && \
	echo "  -> Notarization complete: $${DMG_PATH}" && \
	echo "" && \
	echo "Verifying Gatekeeper assessment..." && \
	spctl --assess --type open --context context:primary-signature "$${DMG_PATH}" 2>&1 && \
	echo "  -> Gatekeeper: OK"
endif

ifeq ($(DETECTED_OS),windows)
check: ## Run Rust compiler checks without building
	@echo "Running Rust compiler checks..."
	cd $(BACKEND); cargo check
	@echo "Rust checks passed"
else
check: ## Run Rust compiler checks without building
	@echo "Running Rust compiler checks..."
	@cd $(BACKEND) && cargo check
	@echo "Rust checks passed"
endif

# ─── Quality ───

ifeq ($(DETECTED_OS),windows)
lint: ## Run Biome linter and Rust clippy
	@echo "Linting frontend code..."
	$(BUN)x biome check .
	@echo "Linting Rust code..."
	cd $(BACKEND); cargo clippy -- -D warnings
	@echo "Lint complete"

lint-fix: ## Run Biome linter with auto-fix
	@echo "Fixing frontend lint issues..."
	$(BUN)x biome check --write .
	@echo "Lint fix complete"

format: ## Format code with Biome and rustfmt
	@echo "Formatting frontend code..."
	$(BUN)x biome format --write .
	@echo "Formatting Rust code..."
	cd $(BACKEND); cargo fmt
	@echo "Format complete"

typecheck: ## Run TypeScript type checking
	@echo "Running TypeScript type checking..."
	cd $(FRONTEND); $(BUN) run typecheck
	@echo "Type check passed"

test: ## Run all tests (Bun + Rust)
	@echo "Running frontend tests..."
	cd $(FRONTEND); $(BUN) test
	@echo "Running Rust tests..."
	cd $(BACKEND); cargo test
	@echo "Tests complete"
else
lint: ## Run Biome linter and Rust clippy
	@echo "Linting frontend code..."
	@$(BUN)x biome check .
	@echo "Linting Rust code..."
	@cd $(BACKEND) && cargo clippy -- -D warnings
	@echo "Lint complete"

lint-fix: ## Run Biome linter with auto-fix
	@echo "Fixing frontend lint issues..."
	@$(BUN)x biome check --write .
	@echo "Lint fix complete"

format: ## Format code with Biome and rustfmt
	@echo "Formatting frontend code..."
	@$(BUN)x biome format --write .
	@echo "Formatting Rust code..."
	@cd $(BACKEND) && cargo fmt
	@echo "Format complete"

typecheck: ## Run TypeScript type checking
	@echo "Running TypeScript type checking..."
	@cd $(FRONTEND) && $(BUN) run typecheck
	@echo "Type check passed"

test: ## Run all tests (Bun + Rust)
	@echo "Running frontend tests..."
	@cd $(FRONTEND) && $(BUN) test
	@echo "Running Rust tests..."
	@cd $(BACKEND) && cargo test
	@echo "Tests complete"
endif

# ─── Versioning ───

ifeq ($(DETECTED_OS),windows)
version: ## Show or set version (use V=X.Y.Z to set)
ifndef V
	@echo "Current version:"
	@(Select-String -Path package.json -Pattern '"version": "(.+)"').Matches.Groups[1].Value
else
	@echo "Updating version to $(V)..."
	@(Get-Content $(BACKEND)\Cargo.toml -Raw) -replace '(?m)^version = ".*"', 'version = "$(V)"' | Set-Content $(BACKEND)\Cargo.toml -NoNewline
	@(Get-Content $(BACKEND)\tauri.conf.json -Raw) -replace '"version": ".*"', '"version": "$(V)"' | Set-Content $(BACKEND)\tauri.conf.json -NoNewline
	@(Get-Content $(FRONTEND)\package.json -Raw) -replace '"version": ".*"', '"version": "$(V)"' | Set-Content $(FRONTEND)\package.json -NoNewline
	@(Get-Content package.json -Raw) -replace '"version": ".*"', '"version": "$(V)"' | Set-Content package.json -NoNewline
	@echo "  -> $(BACKEND)/Cargo.toml"
	@echo "  -> $(BACKEND)/tauri.conf.json"
	@echo "  -> $(FRONTEND)/package.json"
	@echo "  -> package.json"
	@echo ""
	@echo "Version updated to $(V)"
endif
else
version: ## Show or set version (use V=X.Y.Z to set)
ifndef V
	@echo "Current version: $$(grep '^\s*\"version\":' package.json | head -1 | sed 's/.*\"version\": \"\(.*\)\".*/\1/')"
else
	@echo "Updating version to $(V)..."
	@$(SED_INPLACE) 's/^version = ".*"/version = "$(V)"/' $(BACKEND)/Cargo.toml
	@$(SED_INPLACE) 's/"version": ".*"/"version": "$(V)"/' $(BACKEND)/tauri.conf.json
	@$(SED_INPLACE) 's/"version": ".*"/"version": "$(V)"/' $(FRONTEND)/package.json
	@$(SED_INPLACE) 's/"version": ".*"/"version": "$(V)"/' package.json
	@echo "  -> $(BACKEND)/Cargo.toml"
	@echo "  -> $(BACKEND)/tauri.conf.json"
	@echo "  -> $(FRONTEND)/package.json"
	@echo "  -> package.json"
	@echo ""
	@echo "Version updated to $(V)"
endif
endif

# ─── Maintenance ───

ifeq ($(DETECTED_OS),windows)
clean: ## Remove build artifacts and dependencies
	@echo "Cleaning build artifacts..."
	if (Test-Path node_modules) { Remove-Item -Recurse -Force node_modules }
	if (Test-Path $(FRONTEND)\dist) { Remove-Item -Recurse -Force $(FRONTEND)\dist }
	if (Test-Path target) { Remove-Item -Recurse -Force target }
	@echo "Cleanup complete"
else
clean: ## Remove build artifacts and dependencies
	@echo "Cleaning build artifacts..."
	@$(RM) node_modules
	@$(RM) $(FRONTEND)/dist
	@$(RM) target
	@echo "Cleanup complete"
endif

# ─── Golden Corpus ───
# The corpus lives in the private Carelaw repository because its documents
# carry the firm's legal text. Override CORPUS if it lives elsewhere.

CORPUS ?= $(HOME)/Carelaw/carelaw-lily/Tests

corpus-update: ## Regenerate the golden corpus snapshots
	@python3 scripts/run_corpus.py --corpus "$(CORPUS)" --update

corpus-check: ## Compare the current engine output to the golden corpus
	@python3 scripts/run_corpus.py --corpus "$(CORPUS)" --check

.DEFAULT_GOAL := help
