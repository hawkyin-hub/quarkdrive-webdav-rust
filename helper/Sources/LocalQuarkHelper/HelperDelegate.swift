// Stage 13b.1: NSXPCListener delegate. Filters incoming connections
// by pid + libproc euid lookup (Authorization.allowCall) before
// exposing the helper implementation. Connections from root or with
// non-allowed mount points are dropped without a reply.
//
// We check at the listener level (before resuming the connection)
// so the rejected process never even gets the exported interface;
// a hostile caller cannot even probe which methods exist.
import Foundation

final class HelperDelegate: NSObject, NSXPCListenerDelegate {

    func listener(_ listener: NSXPCListener,
                  shouldAcceptNewConnection newConnection: NSXPCConnection) -> Bool {
        let pid = newConnection.processIdentifier
        if !Authorization.allowCall(pid: pid,
                                    operation: "connect",
                                    mountPoint: nil) {
            return false
        }
        newConnection.exportedInterface = NSXPCInterface(with: LOQHelperProtocol.self)
        newConnection.exportedObject = HelperImpl()
        newConnection.resume()
        return true
    }
}
