// Stage 13b.1: build-time version constant. Bumped when the helper
// binary changes semantically (XPC interface change, new mount
// method, etc). install-helper.sh reads this via `client version` and
// reinstalls when it differs from the on-disk version.
//
// Stage 14.1 (localquark-rust): bumped 0.1.1 -> 0.2.0 to add
// chmodMount XPC method (chmod 0755 + chown user:group the
// webdavfs mount point so the user session can ls/cp/edit). Without
// the bump, install-helper.sh sees the on-disk version == built
// version and exits no-op, so the new XPC method is never deployed.
//
// Stage 14.2 (localquark-rust): bumped 0.2.0 -> 0.3.0 to add
// trustCert XPC method (security add-trusted-cert on the System
// keychain for the rust webdav server's self-signed cert, so the
// built-in webdavfs_agent will accept the HTTPS URL).
import Foundation

public enum HelperVersion {
    public static let current = "0.3.0"
}
