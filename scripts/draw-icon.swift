import AppKit

// Source-drawn artwork: a warm cup, a friendly face, and three wisps of steam.
let image = NSImage(size: NSSize(width: 1024, height: 1024))
image.lockFocus()
let context = NSGraphicsContext.current!.cgContext
context.scaleBy(x: 4, y: 4)
NSColor(calibratedRed: 0.96, green: 0.91, blue: 0.81, alpha: 1).setFill()
NSBezierPath(roundedRect: NSRect(x: 5, y: 5, width: 246, height: 246), xRadius: 55, yRadius: 55).fill()
let coffee = NSColor(calibratedRed: 0.47, green: 0.29, blue: 0.21, alpha: 1)
func stroke(_ points: [NSPoint], width: CGFloat, color: NSColor = coffee) {
    let path = NSBezierPath()
    path.move(to: points[0])
    for point in points.dropFirst() { path.line(to: point) }
    path.lineWidth = width
    path.lineCapStyle = .round
    path.lineJoinStyle = .round
    color.setStroke()
    path.stroke()
}
coffee.setStroke()
let handle = NSBezierPath(ovalIn: NSRect(x: 162, y: 83, width: 44, height: 55))
handle.lineWidth = 11
handle.stroke()
let body = NSBezierPath()
body.move(to: NSPoint(x: 59, y: 145))
body.line(to: NSPoint(x: 179, y: 145))
body.line(to: NSPoint(x: 169, y: 77))
body.curve(to: NSPoint(x: 69, y: 77), controlPoint1: NSPoint(x: 143, y: 53), controlPoint2: NSPoint(x: 95, y: 53))
body.close()
NSColor(calibratedRed: 0.85, green: 0.61, blue: 0.45, alpha: 1).setFill()
body.fill()
coffee.setStroke()
body.lineWidth = 4
body.stroke()
coffee.setFill()
NSBezierPath(ovalIn: NSRect(x: 59, y: 135, width: 120, height: 22)).fill()
stroke([NSPoint(x: 47, y: 57), NSPoint(x: 75, y: 49), NSPoint(x: 163, y: 49), NSPoint(x: 190, y: 57)], width: 4)
stroke([NSPoint(x: 97, y: 111), NSPoint(x: 97, y: 104)], width: 5)
stroke([NSPoint(x: 141, y: 111), NSPoint(x: 141, y: 104)], width: 5)
let smile = NSBezierPath()
smile.move(to: NSPoint(x: 109, y: 93))
smile.curve(to: NSPoint(x: 129, y: 93), controlPoint1: NSPoint(x: 115, y: 85), controlPoint2: NSPoint(x: 123, y: 85))
smile.lineWidth = 3
smile.lineCapStyle = .round
smile.stroke()
for x in [91.0, 122.0, 153.0] {
    let steam = NSBezierPath()
    steam.move(to: NSPoint(x: x, y: 174))
    steam.curve(to: NSPoint(x: x, y: 215), controlPoint1: NSPoint(x: x - 16, y: 187), controlPoint2: NSPoint(x: x + 16, y: 200))
    steam.lineWidth = 4
    steam.lineCapStyle = .round
    steam.stroke()
}
image.unlockFocus()
let bitmap = NSBitmapImageRep(data: image.tiffRepresentation!)!
try bitmap.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: CommandLine.arguments[1]))
