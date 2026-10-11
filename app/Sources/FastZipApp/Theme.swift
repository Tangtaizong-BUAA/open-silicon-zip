import SwiftUI
import AppKit
import FastZipKit

/// Visual language: system typography, generous whitespace, one restrained accent,
/// materials instead of heavy fills, and motion limited to short eases.
enum Theme {
    static let corner: CGFloat = 14
    static let pad: CGFloat = 24
    static let gap: CGFloat = 16
    static let ease = Animation.easeInOut(duration: 0.18)

    static let cardFill = Color(nsColor: NSColor(name: nil) { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? NSColor(white: 1, alpha: 0.05)
            : NSColor(white: 0, alpha: 0.035)
    })

    static let hairline = Color(nsColor: NSColor(name: nil) { appearance in
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua
            ? NSColor(white: 1, alpha: 0.10)
            : NSColor(white: 0, alpha: 0.08)
    })

    static func accent(_ prefs: Preferences, dark: Bool) -> Color {
        Color(hex: dark ? prefs.view.accentDark : prefs.view.accentLight) ?? .accentColor
    }
}

extension Color {
    init?(hex: String) {
        var s = hex.trimmingCharacters(in: .whitespaces)
        if s.hasPrefix("#") { s.removeFirst() }
        guard s.count == 6, let v = UInt32(s, radix: 16) else { return nil }
        self.init(.sRGB,
                  red: Double((v >> 16) & 0xFF) / 255,
                  green: Double((v >> 8) & 0xFF) / 255,
                  blue: Double(v & 0xFF) / 255,
                  opacity: 1)
    }
}

struct CardStyle: ViewModifier {
    var highlighted = false
    func body(content: Content) -> some View {
        content
            .background(
                RoundedRectangle(cornerRadius: Theme.corner, style: .continuous)
                    .fill(highlighted ? Color.accentColor.opacity(0.10) : Theme.cardFill)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Theme.corner, style: .continuous)
                    .strokeBorder(highlighted ? Color.accentColor.opacity(0.6) : Theme.hairline, lineWidth: 1)
            )
            .animation(Theme.ease, value: highlighted)
    }
}

extension View {
    func card(highlighted: Bool = false) -> some View { modifier(CardStyle(highlighted: highlighted)) }
}

/// Thin progress bar with a rounded track, used instead of the stock bar for a calmer look.
struct SlimProgress: View {
    var fraction: Double?
    var tint: Color = .accentColor

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.hairline)
                if let f = fraction {
                    Capsule().fill(tint)
                        .frame(width: max(6, geo.size.width * f))
                        .animation(Theme.ease, value: f)
                } else {
                    Capsule().fill(tint.opacity(0.5)).frame(width: geo.size.width * 0.3)
                        .modifier(IndeterminateSlide(width: geo.size.width))
                }
            }
        }
        .frame(height: 6)
        .accessibilityElement()
        .accessibilityLabel("Progress")
        .accessibilityValue(fraction.map { Format.percent($0) } ?? "In progress")
    }
}

private struct IndeterminateSlide: ViewModifier {
    let width: CGFloat
    @State private var phase = false
    func body(content: Content) -> some View {
        content
            .offset(x: phase ? width * 0.7 : 0)
            .onAppear {
                withAnimation(.easeInOut(duration: 1.1).repeatForever(autoreverses: true)) { phase = true }
            }
    }
}
