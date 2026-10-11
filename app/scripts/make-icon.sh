#!/bin/bash
# Renders AppIcon.icns with CoreGraphics (no external deps): a modern minimal
# rounded square with a stylized zipper+bolt mark.
set -euo pipefail
APP="$1"
ICONSET=$(mktemp -d)/AppIcon.iconset
mkdir -p "$ICONSET"

swift - "$ICONSET" <<'SWIFT'
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers
import Foundation

let dir = CommandLine.arguments[1]

func draw(size: CGFloat) -> CGImage {
    let c = CGContext(data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8,
                      bytesPerRow: 0, space: CGColorSpace(name: CGColorSpace.sRGB)!,
                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
    let s = size
    // background: vertical indigo→blue gradient rounded square (macOS squircle-ish)
    let colors = [CGColor(srgbRed: 0.29, green: 0.25, blue: 0.95, alpha: 1),
                  CGColor(srgbRed: 0.10, green: 0.45, blue: 0.98, alpha: 1)] as CFArray
    let grad = CGGradient(colorsSpace: nil, colors: colors, locations: [0, 1])!
    let r = CGRect(x: 0, y: 0, width: s, height: s).insetBy(dx: s*0.04, dy: s*0.04)
    let path = CGPath(roundedRect: r, cornerWidth: s*0.225, cornerHeight: s*0.225, transform: nil)
    c.saveGState(); c.addPath(path); c.clip()
    c.drawLinearGradient(grad, start: CGPoint(x: 0, y: s), end: CGPoint(x: s, y: 0), options: [])
    // subtle highlight
    c.setFillColor(CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.10))
    c.fillEllipse(in: CGRect(x: -s*0.2, y: s*0.55, width: s*1.4, height: s*0.9))
    c.restoreGState()

    let u = s / 100.0  // unit
    // archive box outline
    c.setLineWidth(6*u)
    c.setLineJoin(.round)
    c.setStrokeColor(CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.95))
    let box = CGPath(roundedRect: CGRect(x: 22*u, y: 16*u, width: 56*u, height: 68*u),
                     cornerWidth: 8*u, cornerHeight: 8*u, transform: nil)
    c.addPath(box); c.strokePath()
    // zipper: alternating teeth down the middle
    c.setFillColor(CGColor(srgbRed: 1, green: 1, blue: 1, alpha: 0.95))
    var y: CGFloat = 84
    while y > 42 {
        c.fill(CGRect(x: 47*u, y: (y-8)*u, width: 6*u, height: 8*u))
        c.fill(CGRect(x: (53)*u, y: (y-12)*u, width: 6*u, height: 8*u))
        y -= 16
    }
    // lightning bolt in the lower half
    let bolt = CGMutablePath()
    bolt.move(to: CGPoint(x: 50*u, y: 22*u))
    bolt.addLine(to: CGPoint(x: 38*u, y: 40*u))
    bolt.addLine(to: CGPoint(x: 47*u, y: 40*u))
    bolt.addLine(to: CGPoint(x: 44*u, y: 52*u))
    bolt.addLine(to: CGPoint(x: 60*u, y: 32*u))
    bolt.addLine(to: CGPoint(x: 51*u, y: 32*u))
    bolt.closeSubpath()
    c.setFillColor(CGColor(srgbRed: 1, green: 0.85, blue: 0.25, alpha: 1))
    c.addPath(bolt); c.fillPath()
    return c.makeImage()!
}

for size in [1024.0, 512.0, 256.0, 128.0, 64.0, 32.0, 16.0] {
    let img = draw(size: size)
    let name = size == 1024.0 ? "icon_512x512@2x.png" : "icon_\(Int(size))x\(Int(size)).png"
    let url = URL(fileURLWithPath: dir).appendingPathComponent(name)
    let dest = CGImageDestinationCreateWithURL(url as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(dest, img, nil)
    CGImageDestinationFinalize(dest)
}
// plain 512@1
let img512 = draw(size: 512)
let u512 = URL(fileURLWithPath: dir).appendingPathComponent("icon_512x512.png")
let d512 = CGImageDestinationCreateWithURL(u512 as CFURL, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(d512, img512, nil)
CGImageDestinationFinalize(d512)
SWIFT

iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/AppIcon.icns"
echo "icon written"
