import BarBudsCore
import SwiftUI

struct BatteryView: View {
    let battery: BatteryStatus
    let hasChargingCase: Bool

    var body: some View {
        HStack(spacing: 8) {
            BatteryGauge(title: "Left", reading: battery.left)
            BatteryGauge(title: "Right", reading: battery.right)
            if hasChargingCase {
                BatteryGauge(title: "Case", reading: battery.chargingCase)
            }
        }
    }
}

private struct BatteryGauge: View {
    let title: String
    /// `nil` when the component isn't reporting (e.g. a bud is in a closed case).
    let reading: BatteryReading?

    var body: some View {
        VStack(spacing: 6) {
            BatteryRing(fraction: Double(reading?.percent ?? 0) / 100, tint: tint) {
                Text(reading.map { "\($0.percent)" } ?? "–")
                    .font(.system(.title3, design: .rounded, weight: .semibold))
                    .monospacedDigit()
            }
            .frame(width: 52, height: 52)
            .opacity(reading == nil ? 0.4 : 1)

            HStack(spacing: 2) {
                if reading?.charging == true {
                    Image(systemName: "bolt.fill")
                }
                Text(title)
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .frame(maxWidth: .infinity)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(title)
        .accessibilityValue(accessibilityValue)
    }

    private var tint: Color {
        guard let reading else { return .secondary }
        return reading.percent <= 20 && !reading.charging ? .red : .green
    }

    private var accessibilityValue: String {
        guard let reading else { return "Not reporting" }
        return reading.charging ? "\(reading.percent) percent, charging" : "\(reading.percent) percent"
    }
}

/// A circular level indicator: a faint full track with a rounded arc on top.
private struct BatteryRing<Label: View>: View {
    let fraction: Double
    let tint: Color
    @ViewBuilder let label: Label

    private let lineWidth: CGFloat = 5

    var body: some View {
        ZStack {
            Circle()
                .stroke(tint.opacity(0.2), lineWidth: lineWidth)
            Circle()
                .trim(from: 0, to: min(max(fraction, 0), 1))
                .stroke(tint, style: StrokeStyle(lineWidth: lineWidth, lineCap: .round))
                .rotationEffect(.degrees(-90))
            label
        }
        .padding(lineWidth / 2)
        .animation(.smooth, value: fraction)
    }
}

#Preview {
    BatteryView(
        battery: BatteryStatus(
            left: BatteryReading(percent: 82, charging: false),
            right: BatteryReading(percent: 15, charging: false),
            chargingCase: BatteryReading(percent: 64, charging: true)
        ),
        hasChargingCase: true
    )
    .padding()
    .frame(width: 300)
}
