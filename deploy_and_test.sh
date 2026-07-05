#!/usr/bin/env bash
set -euo pipefail

echo "=== Starting deployment and test process ==="

# Step 1: Build the application
echo "Building application..."
cd /Users/HawkSept/myproject/myapp/localquark-rust
./scripts/build-app.sh

# Step 2: Install to system
echo "Installing to /Applications..."
sudo rm -rf /Applications/LocalQuark-rust.app
sudo cp -R dist/LocalQuark-rust.app /Applications/
sudo chown -R $(whoami):staff /Applications/LocalQuark-rust.app

# Step 3: Kill related old processes
echo "Stopping existing processes..."
killall -9 quarkdrive-webdav run-localquark.sh 2>/dev/null || true
sleep 2

# Force unmount any existing mounts
diskutil unmount force /Volumes/LocalQuark 2>/dev/null || true
sleep 1

# Clear cookies.json 2>/dev/null || true  # Clear cookies to force fresh auth
rm -f ~/Library/Application\ Support/LocalQuark/cookies.json

# Step 4: Start the application
echo "Starting LocalQuark-rust application..."
open -a LocalQuark-rust

echo "Application started. Waiting for initialization..."
sleep 15

# Check if mounted
if mount | grep -q "LocalQuark"; then
    echo "✓ Successfully mounted at /Volumes/LocalQuark"
    ls -la /Volumes/LocalQuark/ | head -5
else
    echo "✗ Mount failed or not yet ready"
    mount | grep -i quark || echo "No Quark mounts found"
fi

echo ""
echo "=== Process complete ==="
echo "Check logs at:"
echo "  - ~/Library/Logs/LocalQuark-rust-launcher.log"
echo "  - ~/Library/Logs/LocalQuark-rust-webdav.log"
