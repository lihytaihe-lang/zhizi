import Carbon
import Foundation
// select <id>  : select that input source (the system must already list it); no argument: print the current source id.
let args = CommandLine.arguments
func source(_ id: String, includeAllInstalled: Bool) -> TISInputSource? {
  guard let list = TISCreateInputSourceList([kTISPropertyInputSourceID as String: id] as CFDictionary, includeAllInstalled)?.takeRetainedValue(),
        let sources = list as? [TISInputSource] else { return nil }
  return sources.first
}
func flag(_ s: TISInputSource, _ key: CFString) -> Bool {
  guard let p = TISGetInputSourceProperty(s, key) else { return false }
  return CFBooleanGetValue(Unmanaged<CFBoolean>.fromOpaque(p).takeUnretainedValue())
}
if args.count >= 3, args[1] == "select" {
  if let s = source(args[2], includeAllInstalled: false) {
    print("select", args[2], TISSelectInputSource(s))
  } else if let s = source(args[2], includeAllInstalled: true) {
    // Listed only among all installed sources (a freshly registered one): the property flags decide whether to try.
    let enabled = flag(s, kTISPropertyInputSourceIsEnabled), selectable = flag(s, kTISPropertyInputSourceIsSelectCapable)
    guard enabled && selectable else {
      print("known to the system but not usable yet (enabled=\(enabled) selectable=\(selectable)): add it in System Settings > Keyboard > Input Sources")
      exit(2)
    }
    print("select (installed list)", args[2], TISSelectInputSource(s))
  } else {
    print("not known to the system yet (not discovered after registering): log out and in, or restart")
    exit(1)
  }
} else {
  let c = TISCopyCurrentKeyboardInputSource().takeRetainedValue()
  let p = TISGetInputSourceProperty(c, kTISPropertyInputSourceID)
  print(Unmanaged<AnyObject>.fromOpaque(p!).takeUnretainedValue() as! String)
}
