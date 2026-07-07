// Stage 13b.1: XPC protocol. The @objc(LOQHelperProtocol) attribute
// pins the Objective-C selector name so NSXPCInterface serializes
// across the Mach port boundary (otherwise Swift's namespacing rules
// would mangle it). All methods take a `withReply` block because
// they're XPC-asynchronous -- the helper is invoked by run-localquark
// from inside the user's launchd agent context.
import Foundation

@objc(LOQHelperProtocol)
public protocol LOQHelperProtocol {
    func ensureDirectory(path: String,
                         withReply: @escaping (Bool, String?) -> Void)

    func mount(url: String,
               mountPoint: String,
               withReply: @escaping (Int32, String?) -> Void)

    func unmount(mountPoint: String,
                 withReply: @escaping (Int32, String?) -> Void)

    // Stage 14.1 (localquark-rust): chmod the mount point so the user
    // can access the root-owned webdavfs mount. After mount_webdav,
    // /Volumes/LocalQuark is root:wheel 755. Without this, Finder and
    // any user-shell `ls` see EACCES. The caller (`main.py`) is the
    // user session, helper runs as root, so this is the natural place
    // for the chown/chmod up. We chmod 0755 and chown to caller's uid:gid.
    // uid/gid are UInt32 because Swift imports uid_t/gid_t as
    // unsigned; the user agent looks them up via getuid()/getgid()
    // which return non-negative values, so the cast is safe.
    func chmodMount(path: String,
                    uid: UInt32,
                    gid: UInt32,
                    withReply: @escaping (Bool, String?) -> Void)

    // Stage 14.2 (localquark-rust): add a PEM cert to the System
    // keychain as a trusted root. macOS's webdavfs_agent refuses to
    // mount an https URL whose server cert is not in System keychain
    // (SecTrustEvaluateIfNecessary -> "Trust evaluate failure:
    // [leaf AnchorTrusted]"). run-localquark.sh generates a self-signed
    // cert on every cold start; trusting it via "security add-trusted-cert
    // -k /Library/Keychains/System.keychain" requires sudo, which the
    // LaunchAgent context cannot satisfy. The helper runs as root in the
    // system LaunchDaemon domain, so it can perform the trust write
    // without prompting. Idempotent: `security add-trusted-cert` returns
    // a non-zero rc if the cert is already trusted and we exit 0
    // regardless (the caller does not need to re-prompt). certPath must
    // be an absolute path; we refuse anything else to keep the attack
    // surface for a hostile caller limited to the cert file the agent
    // itself just generated.
    func trustCert(certPath: String,
                   withReply: @escaping (Int32, String?) -> Void)
}
