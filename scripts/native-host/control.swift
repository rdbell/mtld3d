import AppKit
import ApplicationServices
import Foundation
let args = CommandLine.arguments
guard args.count >= 3, let pid = Int32(args[1]) else { exit(64) }
let app = AXUIElementCreateApplication(pid)
func attr(_ el: AXUIElement, _ name: String) -> CFTypeRef? {
 var value: CFTypeRef?
 guard AXUIElementCopyAttributeValue(el, name as CFString, &value) == .success else { return nil }
 return value
}
let windows = attr(app, kAXWindowsAttribute) as? [AXUIElement] ?? []
if args[2] == "state" {
 print("active",NSRunningApplication(processIdentifier:pid)?.isActive ?? false, "frontmost",NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1, "modifiers",CGEventSource.flagsState(.combinedSessionState).rawValue)
 if let focused=attr(app,kAXFocusedWindowAttribute) { print("focused",attr(focused as! AXUIElement,kAXTitleAttribute) ?? "none") }
 exit(0)
}
if args[2] == "list" {
 let cg = CGWindowListCopyWindowInfo([.optionAll, .excludeDesktopElements], 0) as? [[String: Any]] ?? []
 let rows = cg.filter { ($0[kCGWindowOwnerPID as String] as? Int32) == pid && !($0[kCGWindowName as String] as? String ?? "").isEmpty }
 print(String(data: try JSONSerialization.data(withJSONObject: rows, options: [.prettyPrinted,.sortedKeys]), encoding: .utf8)!)

 exit(0)
}
if args[2] == "pixels" {
 guard args.count == 4, let data=try? Data(contentsOf:URL(fileURLWithPath:args[3])), let rep=NSBitmapImageRep(data:data) else {exit(1)}
 let coordinates=[(0.25,0.25),(0.75,0.25),(0.25,0.75),(0.75,0.75)]
 let expected=[(1.0,0.0,0.0),(0.0,1.0,0.0),(0.0,0.0,1.0),(1.0,1.0,0.0)]
 for (i,pos) in coordinates.enumerated() {
  let pixelX = Int(Double(rep.pixelsWide)*pos.0)
  let pixelY = Int(Double(rep.pixelsHigh)*pos.1)
  guard let color=rep.colorAt(x:pixelX,y:pixelY)?.usingColorSpace(NSColorSpace.deviceRGB) else {exit(1)}
  let want=expected[i]
  guard abs(color.redComponent-want.0)<0.4,abs(color.greenComponent-want.1)<0.4,abs(color.blueComponent-want.2)<0.4 else {print("wrong quadrant",i,color);exit(1)}
 }
 print("four quadrants verified",rep.pixelsWide,rep.pixelsHigh);exit(0)
}
if args[2] == "key" {
 guard NSRunningApplication(processIdentifier:pid)?.isActive == true else {print("refusing key to inactive probe");exit(1)}
 guard args.count == 5, let code = UInt16(args[3]) else {exit(64)}
 let flags: CGEventFlags = args[4] == "command" ? .maskCommand : []
 for down in [true,false] {
  guard let event = CGEvent(keyboardEventSource:nil,virtualKey:code,keyDown:down) else {exit(1)}
  event.flags=flags; event.post(tap:.cghidEventTap); usleep(30000)
 }
 exit(0)
}
if args[2] == "click" {
 guard args.count == 5, let x=Double(args[3]), let y=Double(args[4]) else {exit(64)}
 let moved=CGEvent(mouseEventSource:nil,mouseType:.mouseMoved,mouseCursorPosition:CGPoint(x:x,y:y),mouseButton:.left)!;moved.flags=[];moved.post(tap:.cghidEventTap)
 usleep(100000)
 for kind in [CGEventType.leftMouseDown, .leftMouseUp] {
  guard let event=CGEvent(mouseEventSource:nil,mouseType:kind,mouseCursorPosition:CGPoint(x:x,y:y),mouseButton:.left) else {exit(1)}
  event.flags=[];event.post(tap:.cghidEventTap);usleep(100000)
 }
 exit(0)
}
if args[2] == "settings-value" {
 guard args.count == 4, let panel=windows.first(where:{(attr($0,kAXTitleAttribute) as? String)=="Graphics Settings"}) else {exit(1)}
 func fields(_ el:AXUIElement) -> [AXUIElement] {
  var result:[AXUIElement]=[]
  if (attr(el,kAXRoleAttribute) as? String)==kAXTextFieldRole {result.append(el)}
  for child in attr(el,kAXChildrenAttribute) as? [AXUIElement] ?? [] { result += fields(child) }
  return result
 }
 guard let field=fields(panel).first else {exit(1)}
 guard let pv=attr(field,kAXPositionAttribute),let sv=attr(field,kAXSizeAttribute) else {exit(1)}
 var pos=CGPoint.zero,size=CGSize.zero
 AXValueGetValue(pv as! AXValue,.cgPoint,&pos);AXValueGetValue(sv as! AXValue,.cgSize,&size)
 let point=CGPoint(x:pos.x+size.width/2,y:pos.y+size.height/2)
 for kind in [CGEventType.mouseMoved,.leftMouseDown,.leftMouseUp] {
  let e=CGEvent(mouseEventSource:nil,mouseType:kind,mouseCursorPosition:point,mouseButton:.left)!;e.flags=[];e.post(tap:.cghidEventTap);usleep(50000)
 }
 usleep(100000)
 for down in [true,false] {
  let e=CGEvent(keyboardEventSource:nil,virtualKey:0,keyDown:down)!;e.flags = .maskCommand;e.post(tap:.cghidEventTap);usleep(30000)
 }
 usleep(100000)
 let chars=Array(args[3].utf16)
 for down in [true,false] {
  let e=CGEvent(keyboardEventSource:nil,virtualKey:0,keyDown:down)!
  e.flags=[]
  chars.withUnsafeBufferPointer { e.keyboardSetUnicodeString(stringLength: chars.count, unicodeString:$0.baseAddress!) }
  e.post(tap:.cghidEventTap);usleep(30000)
 }
 usleep(100000)
 for down in [true,false] {
  let e=CGEvent(keyboardEventSource:nil,virtualKey:36,keyDown:down)!;e.flags=[];e.post(tap:.cghidEventTap);usleep(30000)
 }
 usleep(150000)
 print("field",attr(field,kAXValueAttribute) ?? "unknown")
 exit(0)
}
let requestedTitle = args[2] == "settings-close" ? "Graphics Settings" : "Final Fantasy XI"
guard let window = windows.first(where: { (attr($0,kAXTitleAttribute) as? String) == requestedTitle }) else { print("host not found");exit(1) }
func set(_ key: String, _ val: CFTypeRef) {
 let e = AXUIElementSetAttributeValue(window,key as CFString,val)
 if e != .success {print(key,e.rawValue);exit(1)}
}
switch args[2] {
case "activate":
 _ = AXUIElementPerformAction(window,kAXRaiseAction as CFString)
 NSRunningApplication(processIdentifier:pid)?.activate(options:[.activateAllWindows,.activateIgnoringOtherApps])
 let end=Date().addingTimeInterval(3)
 while Date()<end && !(NSRunningApplication(processIdentifier:pid)?.isActive ?? false) { RunLoop.current.run(until:Date().addingTimeInterval(0.05)) }
 guard NSRunningApplication(processIdentifier:pid)?.isActive == true else {exit(1)}
case "resize":
 guard args.count == 7 else {exit(64)}
 var p = CGPoint(x:Double(args[3])!,y:Double(args[4])!)
 var s = CGSize(width:Double(args[5])!,height:Double(args[6])!)
 set(kAXPositionAttribute,AXValueCreate(.cgPoint,&p)!)
 set(kAXSizeAttribute,AXValueCreate(.cgSize,&s)!)
case "minimize": set(kAXMinimizedAttribute,kCFBooleanTrue)
case "restore": set(kAXMinimizedAttribute,kCFBooleanFalse)
case "is-fullscreen": print(attr(window,"AXFullScreen") ?? false)
case "fullscreen": set("AXFullScreen",kCFBooleanTrue)
case "windowed": set("AXFullScreen",kCFBooleanFalse)
case "close", "settings-close":
 guard let b = attr(window,kAXCloseButtonAttribute) else {exit(1)}
 let e=AXUIElementPerformAction(b as! AXUIElement,kAXPressAction as CFString)
 if e != .success {exit(1)}
default: exit(64)
}
