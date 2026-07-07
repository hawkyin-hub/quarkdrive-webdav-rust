// Stage 13b.1: dual-mode entry point.
//
//   server mode (default, no argv[1]): listens on Mach service
//     "com.localquark.webdav-helper.xpc". launchd launches us via
//     /Library/LaunchDaemons/com.localquark.webdav-helper.plist.
//
//   client mode (argv[1] == "client"): connects to that Mach service
//     and invokes a single method, then exits with the method's rc.
//     Used by bin/helper-client.sh from inside run-localquark /
//     teardown-localquark in the user's launchd agent context.
import Foundation

let args = CommandLine.arguments
if args.count > 1 && args[1] == "client" {
    runClient(args: args)
} else {
    runServer()
}

// MARK: - Server

// Held strongly because NSXPCListener.delegate is weak; without this
// the delegate would deallocate immediately and listener.delegate
// would read as nil, causing shouldAcceptNewConnection to never
// fire.
var serverDelegate: HelperDelegate?

func runServer() {
    NSLog("[helper] starting server (version=%@)", HelperVersion.current)
    // Mach service name is registered via launchd plist (MachServices
    // key). NSXPCListener(machServiceName:) wires the helper to that
    // pre-registered port; the deprecated NSXPCListener.service(name:)
    // static is iOS-only.
    let listener = NSXPCListener(machServiceName: "com.localquark.webdav-helper.xpc")
    let delegate = HelperDelegate()
    serverDelegate = delegate
    listener.delegate = delegate
    listener.resume()
    RunLoop.main.run()
}

// MARK: - Client

func runClient(args: [String]) {
    // argv: LocalQuarkHelper client <op> [args...]
    // argv[0] = path
    // argv[1] = "client"
    // argv[2] = op
    guard args.count >= 3 else {
        writeStderr("usage: LocalQuarkHelper client <mount|unmount|mkdir|chmod|trust-cert|version> [args...]\n")
        exit(2)
    }
    let op = args[2]
    if op == "version" {
        print(HelperVersion.current)
        return
    }

    // 13b.1 hotfix: client mode used to call
    //   NSXPCConnection(serviceName: "com.localquark.webdav-helper.xpc")
    // which is the iOS-style initializer. On macOS that goes through
    // bootstrap_look_up2 in the caller's per-UID sub-domain, so it
    // CANNOT see the Mach service that the system-domain LaunchDaemon
    // (registered via plist MachServices) attaches to the system
    // bootstrap context. Symptom: client side acquires a
    // remoteObjectProxy, dispatches a method, then blocks forever in
    // sem.wait(); server-side `sample` shows HelperDelegate is never
    // called (server RunLoop just sits on mach_msg). Confirmed during
    // 13b.2 bring-up (2026-06-30). Fix: use the macOS init with
    // .privileged, which is the documented way for an unprivileged
    // user agent to reach a system-domain launchd Mach service.
    let conn = NSXPCConnection(
        machServiceName: "com.localquark.webdav-helper.xpc",
        options: [.privileged]
    )
    conn.remoteObjectInterface = NSXPCInterface(with: LOQHelperProtocol.self)
    conn.resume()
    guard let proxy = conn.remoteObjectProxy as? LOQHelperProtocol else {
        writeStderr("helper: failed to acquire XPC proxy (helper not installed or running?)\n")
        conn.invalidate()
        exit(1)
    }

    let sem = DispatchSemaphore(value: 0)
    var exitCode: Int32 = 0
    var capturedErr: String?

    switch op {
    case "mkdir":
        guard args.count >= 4 else {
            writeStderr("usage: LocalQuarkHelper client mkdir <path>\n")
            exit(2)
        }
        let path = args[3]
        proxy.ensureDirectory(path: path) { ok, err in
            if !ok {
                capturedErr = err ?? "mkdir failed"
                exitCode = 1
            }
            sem.signal()
        }

    case "mount":
        guard args.count >= 5 else {
            writeStderr("usage: LocalQuarkHelper client mount <url> <mountPoint>\n")
            exit(2)
        }
        let url = args[3]
        let mp = args[4]
        // Defense-in-depth: re-check euid + mountPoint at call site too.
        // The helper itself checks again; this is just so a misconfigured
        // run-localquark fails fast instead of contacting the daemon.
        if mp != Authorization.allowedMountPoint {
            writeStderr("helper client: refusing mount mountPoint=\(mp)\n")
            exit(2)
        }
        proxy.mount(url: url, mountPoint: mp) { rc, err in
            exitCode = rc
            capturedErr = err
            sem.signal()
        }

    case "unmount":
        guard args.count >= 4 else {
            writeStderr("usage: LocalQuarkHelper client unmount <mountPoint>\n")
            exit(2)
        }
        let mp = args[3]
        if mp != Authorization.allowedMountPoint {
            writeStderr("helper client: refusing unmount mountPoint=\(mp)\n")
            exit(2)
        }
        proxy.unmount(mountPoint: mp) { rc, err in
            exitCode = rc
            capturedErr = err
            sem.signal()
        }

    case "chmod":
        // usage: LocalQuarkHelper client chmod <mountPoint> <uid> <gid>
        // Loosens the root-owned webdavfs mount so the user session
        // can ls/cp/mv/edit inside Finder. uid/gid passed by caller
        // (the user agent already knows its own uid from $UID).
        guard args.count >= 6 else {
            writeStderr("usage: LocalQuarkHelper client chmod <path> <uid> <gid>\n")
            exit(2)
        }
        let path = args[3]
        // helper protocol declares uid/gid as UInt32 (uid_t/gid_t).
        // getuid()/getgid() in the caller always return non-negative
        // so this round-trip is lossless.
        guard let uid = UInt32(args[4]), let gid = UInt32(args[5]) else {
            writeStderr("helper client: bad uid/gid '\(args[4])' '\(args[5])'\n")
            exit(2)
        }
        proxy.chmodMount(path: path, uid: uid, gid: gid) { ok, err in
            if !ok { capturedErr = err ?? "chmodMount failed"; exitCode = 1 }
            sem.signal()
        }

    case "trust-cert":
        // usage: LocalQuarkHelper client trust-cert <certPath>
        // Stage 14.2 (localquark-rust): add a PEM cert to the System
        // keychain as a trusted root so webdavfs_agent will accept
        // the rust webdav server's self-signed cert. The helper
        // whitelists allowed paths (HelperImpl.allowedTrustCertPathPrefixes)
        // so a hostile user agent cannot ask helper to trust an
        // arbitrary cert. The cert path is passed by run-localquark.sh
        // which knows exactly where setup-tls.sh wrote the cert.
        guard args.count >= 4 else {
            writeStderr("usage: LocalQuarkHelper client trust-cert <certPath>\n")
            exit(2)
        }
        let certPath = args[3]
        proxy.trustCert(certPath: certPath) { rc, err in
            exitCode = rc
            capturedErr = err
            sem.signal()
        }

    default:
        writeStderr("helper client: unknown op '\(op)'\n")
        conn.invalidate()
        exit(2)
    }

    sem.wait()
    if exitCode != 0 {
        if let err = capturedErr, !err.isEmpty {
            writeStderr("helper client: \(op) failed rc=\(exitCode): \(err)\n")
        } else {
            writeStderr("helper client: \(op) failed rc=\(exitCode)\n")
        }
    }
    conn.invalidate()
    exit(exitCode)
}

private func writeStderr(_ s: String) {
    FileHandle.standardError.write(Data(s.utf8))
}
