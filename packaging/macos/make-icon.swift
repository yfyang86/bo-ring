// Renders the Bo-Ring app icon (1024×1024 PNG) with CoreGraphics:
// a dark rounded square holding a segmented ring with one lit slot.
// usage: swift make-icon.swift out.png
import AppKit

let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "icon.png"
let size: CGFloat = 1024
let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(size), pixelsHigh: Int(size),
                           bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false,
                           colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
NSGraphicsContext.saveGraphicsState()
NSGraphicsContext.current = NSGraphicsContext(bitmapImageRep: rep)
let ctx = NSGraphicsContext.current!.cgContext

// Background squircle (Big Sur grid: 824 px tile, 100 px margin).
let tile = CGRect(x: 100, y: 100, width: 824, height: 824)
let bg = CGPath(roundedRect: tile, cornerWidth: 185, cornerHeight: 185, transform: nil)
ctx.addPath(bg)
ctx.clip()
let grad = CGGradient(colorsSpace: CGColorSpaceCreateDeviceRGB(),
                      colors: [CGColor(red: 0.10, green: 0.12, blue: 0.17, alpha: 1),
                               CGColor(red: 0.03, green: 0.04, blue: 0.07, alpha: 1)] as CFArray,
                      locations: [0, 1])!
ctx.drawLinearGradient(grad, start: CGPoint(x: 512, y: 924), end: CGPoint(x: 512, y: 100), options: [])
ctx.resetClip()

// Segmented ring.
let c = CGPoint(x: 512, y: 512)
let outer: CGFloat = 300, inner: CGFloat = 170
let n = 8
let step = 2 * CGFloat.pi / CGFloat(n)
let gap: CGFloat = 0.06
let teal = CGColor(red: 0.0, green: 0.84, blue: 0.69, alpha: 1)
for i in 0..<n {
    let mid = CGFloat.pi / 2 - CGFloat(i) * step          // slot 0 at 12 o'clock, clockwise
    let a0 = mid + step / 2 - gap, a1 = mid - step / 2 + gap
    let p = CGMutablePath()
    p.addArc(center: c, radius: outer, startAngle: a0, endAngle: a1, clockwise: true)
    p.addArc(center: c, radius: inner, startAngle: a1, endAngle: a0, clockwise: false)
    p.closeSubpath()
    ctx.addPath(p)
    if i == 1 {
        ctx.setShadow(offset: .zero, blur: 40, color: teal)
        ctx.setFillColor(teal)
    } else {
        ctx.setShadow(offset: .zero, blur: 0, color: nil)
        ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 0.13))
    }
    ctx.fillPath()
}
// Hub.
ctx.setShadow(offset: .zero, blur: 0, color: nil)
ctx.setFillColor(CGColor(red: 1, green: 1, blue: 1, alpha: 0.92))
ctx.fillEllipse(in: CGRect(x: c.x - 62, y: c.y - 62, width: 124, height: 124))
ctx.setFillColor(teal)
ctx.fillEllipse(in: CGRect(x: c.x - 26, y: c.y - 26, width: 52, height: 52))

NSGraphicsContext.restoreGraphicsState()
try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: out))
print(out)
