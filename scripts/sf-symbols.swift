// Renders the SF Symbols the app uses into src/renderer/assets/sf/*.png.
// Run with `npm run icons` on a Mac. Each symbol is drawn black at one point size and weight, centered in the same
// square box, at 4x so it stays sharp. The renderer colors it with currentColor via a mask (components/Icon.tsx).
// `name:RRGGBB` writes a pre-tinted copy as `name-RRGGBB.png`, for CSS backgrounds that cannot take currentColor.
import AppKit

let names = CommandLine.arguments.dropFirst(2)
let out = CommandLine.arguments[1]
let box: CGFloat = 20, scale: CGFloat = 4, point: CGFloat = 15

for arg in names {
  let parts = arg.split(separator: ":").map(String.init)
  let name = parts[0]
  let hex = parts.count > 1 ? parts[1] : nil
  var tint = NSColor.black
  if let hex, let v = UInt32(hex, radix: 16) {
    tint = NSColor(srgbRed: CGFloat((v >> 16) & 255) / 255, green: CGFloat((v >> 8) & 255) / 255, blue: CGFloat(v & 255) / 255, alpha: 1)
  }
  guard let base = NSImage(systemSymbolName: name, accessibilityDescription: nil) else {
    FileHandle.standardError.write("missing symbol: \(name)\n".data(using: .utf8)!); exit(1)
  }
  let cfg = NSImage.SymbolConfiguration(pointSize: point * scale, weight: .regular)
  let img = base.withSymbolConfiguration(cfg)!
  let px = Int(box * scale)
  let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: px, pixelsHigh: px, bitsPerSample: 8,
                             samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB,
                             bytesPerRow: 0, bitsPerPixel: 0)!
  NSGraphicsContext.saveGraphicsState()
  NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
  let s = img.size
  let r = NSRect(x: (CGFloat(px) - s.width) / 2, y: (CGFloat(px) - s.height) / 2, width: s.width, height: s.height)
  img.draw(in: r)
  tint.set()
  r.fill(using: .sourceAtop)
  NSGraphicsContext.restoreGraphicsState()
  try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: "\(out)/\(name)\(hex.map { "-" + $0 } ?? "").png"))
}
