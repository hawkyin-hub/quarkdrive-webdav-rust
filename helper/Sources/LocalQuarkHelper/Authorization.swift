// Stage 13b.1: caller authorization via pid + libproc.
//
// NSXPCConnection's auditToken property is not exposed in the Swift
// Foundation overlay shipped with the macOS 13 SDK we are building
// against, so we go through the calling pid. The pid is set by
// launchd at XPC accept time and is not forgeable from a non-root
// caller -- root is the only identity that can ptrace a launchd-
// managed process. We then resolve the euid via libproc
// (proc_pidinfo + PROC_PIDTBSDINFO) so helper knows who is asking.
//
// Two checks:
//
//   1. euid must not be 0. Helper itself runs as root (LaunchDaemon);
//      any root caller is either helper (which never calls its own
//      exported methods) or a hostile root shell. Rejecting root
//      means root must drop to a user context before asking.
//
//   2. mountPoint, when supplied, must equal /Volumes/LocalQuark.
//      This stops a compromised process from asking helper to mount
//      attacker-chosen URLs to arbitrary paths with root privileges.
//      We trust the local 127.0.0.1:8443 webdav server because that
//      is the only URL run-localquark / teardown-localquark ever
//      pass to helper.
import Foundation
import Darwin

enum Authorization {
    // mount/unmount/chmodMount must operate on the LocalQuark volume.
    // Hard-coded, not configurable -- the helper runs as root and we
    // don't want a compromised user agent asking us to chmod arbitrary
    // paths.
    static let allowedMountPoint = "/Volumes/LocalQuark"

    /// Same path guard for chmodMount. Kept as a separate constant so
    /// future relaxations (e.g. README volumes) only touch this enum.
    static let allowedChmodPaths: Set<String> = [allowedMountPoint]

    /// Returns the euid of `pid`, or nil if the process is gone or
    /// the lookup failed. libproc is the supported macOS API for
    /// per-process credential lookup (no audit_token needed).
    static func euid(forPID pid: pid_t) -> uid_t? {
        guard pid > 0 else { return nil }
        var info = proc_bsdinfo()
        let size = Int32(MemoryLayout<proc_bsdinfo>.size)
        let ret = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, size)
        if ret == size {
            return info.pbi_uid
        }
        return nil
    }

    /// Caller must be a non-root process and (for mount/unmount) the
    /// path must be the LocalQuark mount point. Returns true if the
    /// call is allowed to proceed.
    static func allowCall(pid: pid_t,
                          operation: String,
                          mountPoint: String?) -> Bool {
        guard let euid = euid(forPID: pid) else {
            NSLog("[helper] refusing %@: cannot resolve euid for pid=%d",
                  operation, pid)
            return false
        }
        if euid == 0 {
            NSLog("[helper] refusing %@ from root (euid=0)", operation)
            return false
        }
        if let mp = mountPoint, mp != allowedMountPoint {
            NSLog("[helper] refusing %@: mountPoint=%@ not allowed",
                  operation, mp)
            return false
        }
        return true
    }

    /// Stage 14.1 (localquark-rust): authorization for chmodMount.
    /// The user-context caller has already been verified as a non-root
    /// euid at XPC accept time (HelperDelegate.listener), so we do
    /// not re-probe the calling pid here -- the method runs inside
    /// helper itself (root) and would otherwise see euid(forPID: -1)
    /// == nil and always deny. We still enforce the path whitelist
    /// against allowedChmodPaths so a compromised user agent cannot
    /// ask helper to chmod arbitrary paths.
    static func allowChmodCall(path: String) -> Bool {
        guard allowedChmodPaths.contains(path) else {
            NSLog("[helper] refusing chmod: path=%@ not in allowedChmodPaths", path)
            return false
        }
        return true
    }
}
