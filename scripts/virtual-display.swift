// A temporary second display for testing Scrawl on more than one screen, through CoreGraphics'
// private CGVirtualDisplay (what BetterDisplay and DeskPad use). It lasts as long as this process.
//
//   swiftc -O scripts/virtual-display.swift -o /tmp/virtual-display
//   /tmp/virtual-display --width 1920 --height 1080 --scale 1 --place left [--seconds 600]
//
// --width/--height are points; --scale 2 makes it HiDPI (twice the pixels). --place puts it
// left of, right of or above the main display. Prints its id and global bounds, then waits for
// SIGINT/SIGTERM (or --seconds) and exits, which removes the display.

import CoreGraphics
import Foundation

@objc protocol VirtualDisplayModeInit {
  @objc(initWithWidth:height:refreshRate:)
  func initWith(width: UInt32, height: UInt32, refreshRate: Double) -> NSObject
}

@objc protocol VirtualDisplayInit {
  @objc(initWithDescriptor:)
  func initWith(descriptor: NSObject) -> NSObject
}

@objc protocol VirtualDisplayApply {
  @objc(applySettings:)
  func apply(_ settings: NSObject) -> Bool
}

func fail(_ message: String) -> Never {
  FileHandle.standardError.write((message + "\n").data(using: .utf8)!)
  exit(1)
}

func allocate(_ name: String) -> NSObject {
  guard let cls = NSClassFromString(name) as? NSObject.Type else { fail("\(name) is unavailable") }
  return cls.perform(NSSelectorFromString("alloc")).takeUnretainedValue() as! NSObject
}

var width: UInt32 = 1920
var height: UInt32 = 1080
var scale: UInt32 = 1
var place = "left"
var seconds: Double = 0
var args = CommandLine.arguments.dropFirst().makeIterator()
while let arg = args.next() {
  switch arg {
  case "--width": width = UInt32(args.next() ?? "") ?? width
  case "--height": height = UInt32(args.next() ?? "") ?? height
  case "--scale": scale = UInt32(args.next() ?? "") ?? scale
  case "--place": place = args.next() ?? place
  case "--seconds": seconds = Double(args.next() ?? "") ?? 0
  default: fail("unknown argument \(arg)")
  }
}

let descriptor = (NSClassFromString("CGVirtualDisplayDescriptor") as? NSObject.Type ?? { fail("no CGVirtualDisplay") }()).init()
descriptor.setValue(DispatchQueue.global(qos: .userInteractive), forKey: "queue")
descriptor.setValue("Scrawl Test \(width)x\(height)@\(scale)x", forKey: "name")
descriptor.setValue(width * scale, forKey: "maxPixelsWide")
descriptor.setValue(height * scale, forKey: "maxPixelsHigh")
// ~ 24" at 1920 pt wide.
let mm = Double(width) * 0.277
descriptor.setValue(NSValue(size: NSSize(width: mm, height: mm * Double(height) / Double(width))), forKey: "sizeInMillimeters")
descriptor.setValue(0x5C5C, forKey: "productID")
descriptor.setValue(0x3A3A, forKey: "vendorID")
descriptor.setValue(UInt32(place.hashValue & 0xFFFF), forKey: "serialNum")

let display = unsafeBitCast(allocate("CGVirtualDisplay"), to: VirtualDisplayInit.self).initWith(descriptor: descriptor)
let mode = unsafeBitCast(allocate("CGVirtualDisplayMode"), to: VirtualDisplayModeInit.self)
  .initWith(width: width, height: height, refreshRate: 60)
let settings = (NSClassFromString("CGVirtualDisplaySettings") as! NSObject.Type).init()
settings.setValue([mode], forKey: "modes")
settings.setValue(scale > 1 ? 1 : 0, forKey: "hiDPI")
guard unsafeBitCast(display, to: VirtualDisplayApply.self).apply(settings) else { fail("applySettings failed") }
guard let id = (display.value(forKey: "displayID") as? NSNumber)?.uint32Value, id != 0 else { fail("no display id") }

// Let the window server bring it up, then place it.
Thread.sleep(forTimeInterval: 1.5)
let main = CGDisplayBounds(CGMainDisplayID())
let size = CGDisplayBounds(id).size
let origin: CGPoint
switch place {
case "left": origin = CGPoint(x: -size.width, y: 0)
case "right": origin = CGPoint(x: main.width, y: 0)
case "above": origin = CGPoint(x: 0, y: -size.height)
default: fail("--place left|right|above")
}
// macOS may still be arranging the new display; retry until it stays where it was put.
for _ in 0..<10 {
  var config: CGDisplayConfigRef?
  CGBeginDisplayConfiguration(&config)
  CGConfigureDisplayOrigin(config, id, Int32(origin.x), Int32(origin.y))
  let error = CGCompleteDisplayConfiguration(config, .forSession)
  Thread.sleep(forTimeInterval: 1.0)
  if CGDisplayBounds(id).origin == origin { break }
  FileHandle.standardError.write("placing display: \(error.rawValue), at \(CGDisplayBounds(id).origin)\n".data(using: .utf8)!)
}

let b = CGDisplayBounds(id)
let pixels = CGDisplayCopyDisplayMode(id).map { "\($0.pixelWidth)x\($0.pixelHeight)" } ?? "?"
print("{\"id\": \(id), \"x\": \(b.origin.x), \"y\": \(b.origin.y), \"width\": \(b.width), \"height\": \(b.height), \"pixels\": \"\(pixels)\"}")
fflush(stdout)

var sources: [DispatchSourceSignal] = []
for sig in [SIGINT, SIGTERM] {
  signal(sig, SIG_IGN)
  let source = DispatchSource.makeSignalSource(signal: sig, queue: .main)
  source.setEventHandler { exit(0) }
  source.resume()
  sources.append(source)
}
if seconds > 0 {
  DispatchQueue.main.asyncAfter(deadline: .now() + seconds) { exit(0) }
}
_ = display
dispatchMain()
