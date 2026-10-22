// Read-only: for the two input sources we care about, print enabled / selectable / selected as the system reports them.
import Carbon
import Foundation
func flag(_ s: TISInputSource, _ k: CFString) -> Int {
  guard let p = TISGetInputSourceProperty(s, k) else { return -1 }
  return CFBooleanGetValue(Unmanaged<CFBoolean>.fromOpaque(p).takeUnretainedValue()) ? 1 : 0
}
let list = TISCreateInputSourceList(nil, false).takeRetainedValue() as! [TISInputSource]
for s in list {
  guard let p = TISGetInputSourceProperty(s, kTISPropertyInputSourceID),
        let id = Unmanaged<AnyObject>.fromOpaque(p).takeUnretainedValue() as? String else { continue }
  if id == "com.hongye.facai.inputmethod.zhizi.pinyin" || id == "com.sogou.inputmethod.sogou.pinyin" {
    print("\(id.split(separator: ".").dropLast().last ?? "?") en=\(flag(s, kTISPropertyInputSourceIsEnabled)) sel=\(flag(s, kTISPropertyInputSourceIsSelected))")
  }
}
