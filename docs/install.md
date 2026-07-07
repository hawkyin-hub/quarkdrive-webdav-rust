# Install

## From release `.app` (recommended)

```bash
# 1. download
open https://github.com/hawkyin-hub/quarkdrive-webdav-rust/releases

# 2. drag LocalQuark-rust.app to /Applications
# 3. bypass Gatekeeper (one-time):
xattr -cr /Applications/LocalQuark-rust.app

# 4. install privileged Helper (one-time, requires sudo):
sudo /Applications/LocalQuark-rust.app/Contents/Resources/bin/install-helper.sh

# 5. sign in to Quark Drive in Chrome / Brave / Arc / Edge
open https://pan.quark.cn

# 6. launch
open /Applications/LocalQuark-rust.app
```

## From source

Requirements:
- macOS 12+ with Xcode Command Line Tools
- Rust 1.74+ (`rustup install stable`)
- A Chrome-family browser signed in to Quark

```bash
git clone https://github.com/hawkyin-hub/quarkdrive-webdav-rust.git
cd quarkdrive-webdav-rust

# Build
cargo build --release -p quarkdrive-webdav

# Package as .app
bash scripts/build-app.sh

# Install
sudo cp -R dist/LocalQuark-rust.app /Applications/
sudo /Applications/LocalQuark-rust.app/Contents/Resources/bin/install-helper.sh

# First run: open the .app; it auto-installs cert + mounts /Volumes/LocalQuark.
open /Applications/LocalQuark-rust.app
```

## Updating

```bash
git pull
cargo build --release -p quarkdrive-webdav
bash scripts/build-app.sh
bash scripts/build_deploy_test.sh   # auto-kills old process + replaces + restarts
```

## Uninstalling

```bash
sudo /Applications/LocalQuark-rust.app/Contents/Resources/bin/uninstall-helper.sh
sudo rm -rf /Applications/LocalQuark-rust.app
rm -rf ~/Library/Application\ Support/LocalQuark
rm -rf ~/Library/Caches/LocalQuark
diskutil unmount force /Volumes/LocalQuark   # if still mounted
```

## Logs

- Launcher: `~/Library/Logs/LocalQuark-rust-launcher.log`
- Server: `~/Library/Logs/LocalQuark-rust-webdav.log`

For continuous tail:
```bash
tail -F ~/Library/Logs/LocalQuark-rust-webdav.log
```

## Upgrading the Helper (browser Cookie access)

Required once per machine. The Helper is a small privileged process installed at:

```
/Library/LaunchDaemons/com.localquark.webdav-helper.plist
/usr/local/libexec/LocalQuark-rust.app/Contents/Resources/helper/com.localquark.webdav-helper
```

If the Helper is missing or the install script fails:
- Check Console.app for `com.localquark.webdav-helper` entries.
- Reinstall: `sudo /Applications/LocalQuark-rust.app/Contents/Resources/bin/install-helper.sh`
- Verify: `sudo launchctl list | grep localquark`
