// Renders the Promptly app icon at 1024x1024 with CoreGraphics.
//   swift packaging/icon/render-icon.swift packaging/icon/promptly-1024.png
//
// Design: a dark macOS squircle; an ember-orange prompt chevron with a soft
// glow, a cursor bar, and a four-point spark — a terminal that's alive.

import AppKit
import CoreGraphics
import Foundation

let size: CGFloat = 1024
let out = CommandLine.arguments.count > 1 ? CommandLine.arguments[1] : "promptly-1024.png"

let cs = CGColorSpace(name: CGColorSpace.displayP3)!
guard let ctx = CGContext(
    data: nil, width: Int(size), height: Int(size), bitsPerComponent: 8, bytesPerRow: 0,
    space: cs, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
) else { fatalError("no context") }

func rgb(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat, _ a: CGFloat = 1) -> CGColor {
    CGColor(colorSpace: cs, components: [r / 255, g / 255, b / 255, a])!
}

// --- Squircle body (Apple grid: 824pt content in a 1024 canvas, ~22% radius)
let inset: CGFloat = 100
let body = CGRect(x: inset, y: inset, width: size - 2 * inset, height: size - 2 * inset)
let radius: CGFloat = 185
let squircle = CGPath(roundedRect: body, cornerWidth: radius, cornerHeight: radius, transform: nil)

// Drop shadow under the tile.
ctx.saveGState()
ctx.setShadow(offset: CGSize(width: 0, height: -18), blur: 40, color: rgb(0, 0, 0, 0.45))
ctx.addPath(squircle)
ctx.setFillColor(rgb(18, 19, 23))
ctx.fillPath()
ctx.restoreGState()

// Body gradient: graphite to near-black.
ctx.saveGState()
ctx.addPath(squircle)
ctx.clip()
let bodyGrad = CGGradient(colorsSpace: cs, colors: [rgb(44, 46, 54), rgb(16, 17, 21)] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(bodyGrad, start: CGPoint(x: size / 2, y: body.maxY), end: CGPoint(x: size / 2, y: body.minY), options: [])

// Ember radial glow behind the glyphs.
let glow = CGGradient(colorsSpace: cs, colors: [rgb(255, 120, 70, 0.42), rgb(255, 90, 50, 0.0)] as CFArray, locations: [0, 1])!
ctx.drawRadialGradient(glow, startCenter: CGPoint(x: 430, y: 470), startRadius: 0, endCenter: CGPoint(x: 430, y: 470), endRadius: 430, options: [])

// Subtle scanlines for the terminal feel.
ctx.setFillColor(rgb(255, 255, 255, 0.025))
var y = body.minY
while y < body.maxY {
    ctx.fill(CGRect(x: body.minX, y: y, width: body.width, height: 3))
    y += 14
}

// Top sheen.
let sheen = CGGradient(colorsSpace: cs, colors: [rgb(255, 255, 255, 0.10), rgb(255, 255, 255, 0)] as CFArray, locations: [0, 1])!
ctx.drawLinearGradient(sheen, start: CGPoint(x: size / 2, y: body.maxY), end: CGPoint(x: size / 2, y: body.maxY - 300), options: [])
ctx.restoreGState()

// Hairline rim.
ctx.addPath(squircle)
ctx.setStrokeColor(rgb(255, 255, 255, 0.10))
ctx.setLineWidth(4)
ctx.strokePath()

// --- Chevron "›" with gradient stroke and glow
let chevron = CGMutablePath()
chevron.move(to: CGPoint(x: 290, y: 690))
chevron.addLine(to: CGPoint(x: 520, y: 500))
chevron.addLine(to: CGPoint(x: 290, y: 310))
let stroked = chevron.copy(strokingWithWidth: 118, lineCap: .round, lineJoin: .round, miterLimit: 10)

ctx.saveGState()
ctx.setShadow(offset: .zero, blur: 60, color: rgb(255, 110, 60, 0.85))
ctx.addPath(stroked)
ctx.setFillColor(rgb(255, 120, 70))
ctx.fillPath()
ctx.restoreGState()

let ember = CGGradient(colorsSpace: cs, colors: [rgb(255, 196, 120), rgb(255, 122, 72), rgb(226, 70, 52)] as CFArray, locations: [0, 0.5, 1])!
ctx.saveGState()
ctx.addPath(stroked)
ctx.clip()
ctx.drawLinearGradient(ember, start: CGPoint(x: 290, y: 700), end: CGPoint(x: 520, y: 300), options: [.drawsBeforeStartLocation, .drawsAfterEndLocation])
ctx.restoreGState()

// --- Cursor bar
let cursor = CGPath(roundedRect: CGRect(x: 590, y: 300, width: 190, height: 74), cornerWidth: 20, cornerHeight: 20, transform: nil)
ctx.saveGState()
ctx.setShadow(offset: .zero, blur: 34, color: rgb(255, 255, 255, 0.35))
ctx.addPath(cursor)
ctx.setFillColor(rgb(242, 242, 246))
ctx.fillPath()
ctx.restoreGState()

// --- Four-point spark (top right)
func spark(center c: CGPoint, r: CGFloat, waist: CGFloat) -> CGPath {
    let p = CGMutablePath()
    let pts: [(CGFloat, CGFloat)] = [(0, r), (waist, waist), (r, 0), (waist, -waist), (0, -r), (-waist, -waist), (-r, 0), (-waist, waist)]
    for (i, (dx, dy)) in pts.enumerated() {
        let pt = CGPoint(x: c.x + dx, y: c.y + dy)
        if i == 0 { p.move(to: pt) } else { p.addLine(to: pt) }
    }
    p.closeSubpath()
    return p
}
ctx.saveGState()
ctx.setShadow(offset: .zero, blur: 40, color: rgb(255, 200, 140, 0.9))
ctx.addPath(spark(center: CGPoint(x: 700, y: 680), r: 112, waist: 20))
ctx.setFillColor(rgb(255, 226, 180))
ctx.fillPath()
ctx.restoreGState()
ctx.addPath(spark(center: CGPoint(x: 806, y: 584), r: 40, waist: 8))
ctx.setFillColor(rgb(255, 170, 120, 0.9))
ctx.fillPath()

// --- Write PNG
let image = ctx.makeImage()!
let rep = NSBitmapImageRep(cgImage: image)
guard let data = rep.representation(using: .png, properties: [:]) else { fatalError("png") }
try! data.write(to: URL(fileURLWithPath: out))
print("wrote \(out)")
