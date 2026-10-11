// Entry point for the hand-assembled FinderSync appex. The legacy
// principal-class bootstrap that NSExtensionMain performs for pluginkit-hosted
// extensions: instantiate NSExtensionPrincipalClass (FIFinderSync subclasses
// register with the FinderSync agent on init) and service the run loop.
import Cocoa
import FinderSync

let principalName = (Bundle.main.infoDictionary?["NSExtension"] as? [String: Any])?["NSExtensionPrincipalClass"] as? String
let className = principalName.map { $0.contains(".") ? $0 : $0 } ?? "FastZipFinderSync.FinderSync"

let cls: AnyClass? = NSClassFromString(className)
    ?? NSClassFromString(className.replacingOccurrences(of: ".", with: "_"))
    ?? NSClassFromString("_TtC17FastZipFinderSync10FinderSync")

guard let objectClass = cls as? NSObject.Type else {
    FileHandle.standardError.write(Data("FastZip appex: principal class not found: \(className)\n".utf8))
    exit(1)
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
_ = objectClass.init()   // FIFinderSync: registers with the host on init
app.run()
