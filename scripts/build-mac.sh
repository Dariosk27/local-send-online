#!/bin/bash
# Builds "Local Send Online.app" and LocalSendOnline.dmg on your own Mac.
# Needs only an Internet connection: installs Rust and Node if missing.
# Usage: from the repository folder,  bash scripts/build-mac.sh
set -euo pipefail
cd "$(dirname "$0")/.."

if ! xcode-select -p >/dev/null 2>&1; then
  echo "Installo gli strumenti da riga di comando di Xcode (serve una conferma a schermo)..."
  xcode-select --install || true
  echo "Al termine dell'installazione rilancia questo script."
  exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "Installo Rust..."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  source "$HOME/.cargo/env"
fi
if ! command -v npx >/dev/null 2>&1; then
  echo "Serve Node.js: installalo da https://nodejs.org (versione LTS) e rilancia lo script."
  exit 1
fi

rustup target add aarch64-apple-darwin x86_64-apple-darwin
cd app/src-tauri
npx -y @tauri-apps/cli@2 build --target universal-apple-darwin

DMG=$(find target/universal-apple-darwin/release/bundle/dmg -name '*.dmg' | head -1)
cp "$DMG" ../../LocalSendOnline.dmg
echo
echo "Fatto: $(cd ../.. && pwd)/LocalSendOnline.dmg"
echo "L'app è anche in: $(pwd)/target/universal-apple-darwin/release/bundle/macos/"
