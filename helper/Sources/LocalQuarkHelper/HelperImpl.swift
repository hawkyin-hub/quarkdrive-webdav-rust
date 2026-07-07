// Stage 13b.1: server-side implementation of LOQHelperProtocol.
// Helper runs as root (LaunchDaemon), so file system and mount
// operations that fail in the user agent context succeed here.
//
// All methods are XPC-asynchronous (withReply block). Run-localquark
// spawns the helper binary in client mode, which connects to the
// Mach service, invokes a method, and waits for the reply via a
// dispatch semaphore.
//
// We do NOT use NSXPCConnection.interruptionHandler /
// invalidationHandler to log: launchd restarts us on crash, and the
// log already goes to launchd's standardErrorPath (configured in
// install-helper.sh). Keep the impl minimal.
import Foundation

final class HelperImpl: NSObject, LOQHelperProtocol {

    // Stage 14.2 (localquark-rust): paths we are willing to trust via
    // trustCert. We restrict trustCert to the cert file the agent
    // itself generated under the *user's* Library/Application
    // Support/LocalQuark/certs/cert.pem (setup-tls.sh's
    // $LOCALQUARK_CERT_DIR/cert.pem). Anything else is refused -- a
    // hostile user agent that gets XPC access cannot trick the helper
    // into trusting arbitrary certs (e.g. an attacker MITM cert for
    // a public site).
    //
    // Note: NSHomeDirectory() inside the helper (running as root) is
    // /var/root, NOT the user\'s $HOME. We resolve the user\'s home
    // by reading $HOME from the process environment (launchd sets
    // HOME=/Users/HawkSept before exec\'ing us), or via
    // getpwuid(getuid()) as a fallback. In practice the launchd
    // LaunchDaemon plist does not set HOME so we go through
    // getpwuid(geteuid()) -- which on a LaunchDaemon returns 0
    // (root), giving us /var/root, which is wrong. We hard-code the
    // user path via /Users/$SUDO_USER; if that is not set we fall
    // back to scanning /Users for the only non-root home directory
    // that has a Library/Application Support/LocalQuark/certs/cert.pem.
    /// Resolve the invoking user's login keychain. Helper runs as
    /// root with HOME=/var/root, so we cannot use NSHomeDirectory().
    /// We try the SUDO_USER env var first, then scan /Users for
    /// the only non-root home that has a Library/Keychains/login.keychain-db.
    private static func userLoginKeychainPath() -> String {
        let env = ProcessInfo.processInfo.environment
        if let user = env["SUDO_USER"], !user.isEmpty, user != "root" {
            return "/Users/" + user + "/Library/Keychains/login.keychain-db"
        }
        if let user = env["LOCALQUARK_USER"], !user.isEmpty, user != "root" {
            return "/Users/" + user + "/Library/Keychains/login.keychain-db"
        }
        let fm = FileManager.default
        if let users = try? fm.contentsOfDirectory(atPath: "/Users") {
            for u in users where u != "Shared" && u != ".localized" {
                let kc = "/Users/" + u + "/Library/Keychains/login.keychain-db"
                if fm.fileExists(atPath: kc) {
                    return kc
                }
            }
        }
        return "/Library/Keychains/System.keychain" // last-resort fallback
    }

    private static let allowedTrustCertPathPrefixes: [String] = {
        if let envHome = ProcessInfo.processInfo.environment["HOME"],
           !envHome.isEmpty,
           envHome != "/var/root" {
            return [envHome + "/Library/Application Support/LocalQuark/certs/cert.pem"]
        }
        // Fallback: scan /Users for a user home with our cert. There
        // is normally exactly one such user on a single-user Mac.
        let fm = FileManager.default
        if let users = try? fm.contentsOfDirectory(atPath: "/Users") {
            for u in users {
                let candidate = "/Users/" + u + "/Library/Application Support/LocalQuark/certs/cert.pem"
                if fm.fileExists(atPath: candidate) {
                    return [candidate]
                }
            }
        }
        // Last-ditch: try NSHomeDirectory() anyway so a developer
        // running helper locally from the shell can still trust.
        return [NSHomeDirectory() + "/Library/Application Support/LocalQuark/certs/cert.pem"]
    }()

    func ensureDirectory(path: String,
                         withReply: @escaping (Bool, String?) -> Void) {
        do {
            try FileManager.default.createDirectory(
                atPath: path,
                withIntermediateDirectories: true,
                attributes: nil
            )
            withReply(true, nil)
        } catch {
            withReply(false, "\(error)")
        }
    }

    func mount(url: String,
               mountPoint: String,
               withReply: @escaping (Int32, String?) -> Void) {
        runProcess(
            launchPath: "/sbin/mount_webdav",
            arguments: ["-s", url, mountPoint],
            withReply: withReply
        )
    }

    func unmount(mountPoint: String,
                 withReply: @escaping (Int32, String?) -> Void) {
        // Try diskutil first (graceful -- handles webdavfs cleanup),
        // fall back to umount -f on failure.
        runProcess(
            launchPath: "/usr/sbin/diskutil",
            arguments: ["unmount", mountPoint]
        ) { rc, err in
            if rc == 0 {
                withReply(0, nil)
                return
            }
            NSLog("[helper] diskutil unmount rc=%d, falling back to umount -f", rc)
            self.runProcess(
                launchPath: "/sbin/umount",
                arguments: ["-f", mountPoint],
                withReply: withReply
            )
        }
    }

    // Stage 14.1 (localquark-rust): chmod 0755 + chown <caller-uid>:<caller-gid>
    // the mount point so the user process can read/list/upload. webdavfs
    // mounts come out as root:wheel 0755; without this the only thing
    // visible in /Volumes is the disk icon, the content reads back EACCES.
    // Authorization.allowCall(pid: , operation: "chmod", mountPoint: <path>)
    // has already gate-checked the path against allowedChmodPaths.
    func chmodMount(path: String,
                    uid: UInt32,
                    gid: UInt32,
                    withReply: @escaping (Bool, String?) -> Void) {
        // Defense in depth: HelperDelegate rejected root / unknown
        // callers at connection time, so we only need to enforce the
        // path whitelist here. We do not re-probe the calling pid
        // because chmodMount runs inside helper (root) and a fresh
        // euid lookup against the helper's own pid would yield
        // euid=0 and trigger allowCall's root rejection.
        if !Authorization.allowChmodCall(path: path) {
            withReply(false, "chmodMount: path \(path) not in allowedChmodPaths")
            return
        }
        // macOS 27 webdavfs_agent mounts come out as root:wheel 0700 and
        // refuse chmod on the mount-point vnode, so we treat chmod as
        // best-effort and always proceed to chown. The errnoPtr pattern
        // was a bug anyway: chmod writes to libc's thread-local errno,
        // not to a buffer we pass in; the helper previously read garbage
        // from errnoPtr.pointee and reported bogus errno values.
        let chmodRc = path.withCString { cstr in
            chmod(cstr, 0o755)
        }
        if chmodRc != 0 {
            NSLog("[helper] chmod(%@, 0755) failed (best-effort, continuing to chown)", path)
        }
        let rc2 = path.withCString { cstr in
            chown(cstr, uid, gid)
        }
        if rc2 != 0 {
            let e = errno
            withReply(false, "chown(\(path), \(uid):\(gid)) failed errno=\(e)")
            return
        }
        withReply(true, nil)
    }

    // Stage 14.2 (localquark-rust): trust a PEM cert in the System
    // keychain so webdavfs_agent (the macOS built-in webdav mounter) will
    // accept the rust webdav server's self-signed cert.
    //
    // Path whitelist (allowedTrustCertPathPrefixes) keeps a hostile user
    // agent from asking helper to trust e.g. an attacker cert for
    // google.com. We only accept certs in $HOME/Library/Application
    // Support/LocalQuark/certs/cert.pem, which is the only file
    // setup-tls.sh writes.
    //
    // `security add-trusted-cert` returns non-zero on "already trusted",
    // which we treat as success (idempotent re-run is fine).
    func trustCert(certPath: String,
                   withReply: @escaping (Int32, String?) -> Void) {
        let normalized = (certPath as NSString).expandingTildeInPath
        let allowed = HelperImpl.allowedTrustCertPathPrefixes.contains { prefix in
            normalized == prefix
        }
        guard allowed else {
            withReply(-1, "trustCert: certPath \(certPath) not in whitelist")
            return
        }
        guard FileManager.default.fileExists(atPath: normalized) else {
            withReply(-1, "trustCert: \(normalized) does not exist")
            return
        }
        // [macOS 27] add SSL anchor trust to user's login keychain.
        // `security add-trusted-cert -p ssl -r trustRoot` writes only
        // a kSecTrustSettingsPolicy (SSL) without
        // kSecTrustSettingsResult=TrustRoot, so SecTrustEvaluate
        // still fails for webdavfs_agent. We must set the
        // kSecTrustSettingsResult explicitly via SecTrustSettingsSetTrustSettings.
        //
        // Helper runs as root, but user keychain is writable by
        // any process that can read the user's authentication. We
        // resolve the invoking user's home via SUDO_USER / HOME /
        // scanning /Users, and use the
        // `kSecTrustSettingsDomain.user` domain so no admin auth
        // is required.
        do {
            let pemData = try Data(contentsOf: URL(fileURLWithPath: normalized))
            guard let pemString = String(data: pemData, encoding: .utf8) else {
                withReply(-1, "trustCert: cert is not valid UTF-8 text")
                return
            }
            let lines = pemString.components(separatedBy: .newlines)
            var base64String = ""
            var insideCert = false
            for line in lines {
                let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
                if trimmed == "-----BEGIN CERTIFICATE-----" {
                    insideCert = true
                    continue
                }
                if trimmed == "-----END CERTIFICATE-----" {
                    insideCert = false
                    break
                }
                if insideCert {
                    base64String += trimmed
                }
            }
            guard let derData = Data(base64Encoded: base64String) else {
                withReply(-1, "trustCert: base64 decoding failed")
                return
            }
            guard let cert = SecCertificateCreateWithData(nil, derData as CFData) else {
                withReply(-1, "trustCert: SecCertificateCreateWithData failed")
                return
            }
            let policy = SecPolicyCreateSSL(true, nil)
            let trustSettings: CFArray = [
                [kSecTrustSettingsPolicy as String: policy,
                 kSecTrustSettingsResult as String: NSNumber(value: SecTrustSettingsResult.trustRoot.rawValue)] as CFDictionary
            ] as CFArray
            // Always operate in user domain so no admin auth required.
            let status = SecTrustSettingsSetTrustSettings(
                cert, SecTrustSettingsDomain.user, trustSettings
            )
            NSLog("[helper] trustCert: SecTrustSettingsSetTrustSettings status=\(status) (cert ref=\(cert))")
            // errSecSuccess or errSecDuplicateItem (25291) both mean
            // the cert is trusted.
            let rc = Int(status)
            if rc == 0 || rc == -25291 {
                withReply(0, nil)
            } else {
                withReply(Int32(rc), "trustCert: SecTrustSettingsSetTrustSettings failed (status=\(rc))")
            }
        } catch {
            withReply(-1, "trustCert: \(error)")
        }
    }

    // MARK: - Private

    private func runProcess(launchPath: String,
                            arguments: [String],
                            withReply: @escaping (Int32, String?) -> Void) {
        let proc = Process()
        proc.launchPath = launchPath
        proc.arguments = arguments
        let errPipe = Pipe()
        proc.standardError = errPipe
        proc.standardOutput = Pipe()
        do {
            try proc.run()
        } catch {
            withReply(-1, "spawn \(launchPath) failed: \(error)")
            return
        }
        proc.waitUntilExit()
        let errData = errPipe.fileHandleForReading.readDataToEndOfFile()
        let errStr = String(data: errData, encoding: .utf8)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if errStr?.isEmpty == true {
            withReply(proc.terminationStatus, nil)
        } else {
            withReply(proc.terminationStatus, errStr)
        }
    }
}
