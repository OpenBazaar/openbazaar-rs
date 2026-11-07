import type { ReactNode } from "react";
import { ActivityIndicator, Pressable, StyleSheet, Text, View, type ViewStyle } from "react-native";

// Colors and small building blocks shared by the native home node screens,
// matching the shop's palette in App.tsx.

export const NAVY = "#063853";
export const BLUE = "#0e6da5";
export const INK = "#242424";
export const MUTED = "#6b6b6b";
export const LINE = "rgba(6, 56, 83, 0.12)";
export const PAGE = "#f5f7f8";
export const CARD = "#ffffff";
export const SOFT = "#e7eef2";
export const ERROR = "#9f2f2f";
export const GOOD = "#1f6f43";

export function Button({ label, onPress, disabled, kind = "primary", busy }: { label: string; onPress: () => void; disabled?: boolean; kind?: "primary" | "plain" | "danger"; busy?: boolean }) {
  return (
    <Pressable
      accessibilityRole="button"
      accessibilityLabel={label}
      disabled={disabled || busy}
      onPress={onPress}
      style={({ pressed }) => [ui.button, kind !== "primary" && ui.buttonPlain, kind === "danger" && ui.buttonDanger, (disabled || busy) && ui.disabled, pressed && ui.pressed]}
    >
      {busy ? <ActivityIndicator color={kind === "primary" ? "#fff" : NAVY} /> : <Text style={[ui.buttonText, kind !== "primary" && ui.buttonTextPlain, kind === "danger" && ui.dangerText]}>{label}</Text>}
    </Pressable>
  );
}

export function Card({ title, children, style }: { title?: string; children: ReactNode; style?: ViewStyle }) {
  return (
    <View style={[ui.card, style]}>
      {title ? <Text style={ui.cardTitle}>{title}</Text> : null}
      {children}
    </View>
  );
}

export function Row({ label, value }: { label: string; value: string }) {
  return (
    <View style={ui.row}>
      <Text style={ui.rowLabel}>{label}</Text>
      <Text style={ui.rowValue} selectable>
        {value}
      </Text>
    </View>
  );
}

export function Notice({ text, tone = "info" }: { text: string; tone?: "info" | "error" }) {
  return <Text style={[ui.notice, tone === "error" && ui.noticeError]}>{text}</Text>;
}

export function formatBtc(sats: number): string {
  const fixed = (sats / 100_000_000).toFixed(8).replace(/0+$/, "").replace(/\.$/, "");
  const [whole, frac = ""] = fixed.split(".");
  return `${whole}.${frac.padEnd(4, "0")} BTC`;
}

export function ago(unixSecs: number | null | undefined): string {
  if (!unixSecs) return "never";
  const secs = Math.max(0, Math.floor(Date.now() / 1000) - unixSecs);
  if (secs < 60) return "just now";
  if (secs < 3600) return `${Math.floor(secs / 60)} min ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)} h ago`;
  return `${Math.floor(secs / 86400)} days ago`;
}

export const ui = StyleSheet.create({
  screen: { padding: 16, gap: 14, paddingBottom: 40 },
  title: { fontSize: 26, fontWeight: "700", color: NAVY },
  subtitle: { fontSize: 16, color: INK, lineHeight: 22 },
  muted: { fontSize: 14, color: MUTED, lineHeight: 20 },
  button: { backgroundColor: NAVY, borderRadius: 10, paddingVertical: 14, paddingHorizontal: 18, alignItems: "center" },
  buttonPlain: { backgroundColor: SOFT },
  buttonDanger: { backgroundColor: "#f6e3e3" },
  buttonText: { color: "#fff", fontSize: 16, fontWeight: "600" },
  buttonTextPlain: { color: NAVY },
  dangerText: { color: ERROR },
  disabled: { opacity: 0.45 },
  pressed: { opacity: 0.8 },
  card: { backgroundColor: CARD, borderRadius: 14, padding: 16, gap: 10, borderWidth: 1, borderColor: LINE },
  cardTitle: { fontSize: 13, fontWeight: "700", color: MUTED, textTransform: "uppercase", letterSpacing: 0.6 },
  row: { flexDirection: "row", justifyContent: "space-between", gap: 12 },
  rowLabel: { fontSize: 15, color: MUTED },
  rowValue: { fontSize: 15, color: INK, flexShrink: 1, textAlign: "right" },
  notice: { fontSize: 14, color: NAVY, backgroundColor: SOFT, padding: 12, borderRadius: 10, lineHeight: 20 },
  noticeError: { color: ERROR, backgroundColor: "#f6e3e3" },
  input: { backgroundColor: CARD, borderWidth: 1, borderColor: LINE, borderRadius: 10, padding: 12, fontSize: 17, color: INK },
});
