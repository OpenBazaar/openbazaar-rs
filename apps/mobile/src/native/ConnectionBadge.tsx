import { ActivityIndicator, StyleSheet, Text, View } from "react-native";

import { useSession } from "./session.ts";
import { GOOD, NAVY, SOFT } from "./ui.tsx";

/** "Connecting privately…" while Tor is not ready or a request is out; "On home Wi-Fi" on the LAN channel. */
export function ConnectionBadge() {
  const session = useSession();
  if (session.status !== "paired") return null;
  const busy = session.inFlight > 0 || session.syncing;
  const lan = session.via === "lan";
  const connecting = busy ? !lan : !session.torReady && !lan;
  const queued = session.queue.pending.length + (session.queue.inFlight?.requests.length ?? 0);
  if (!connecting && !lan && !queued) return null;
  return (
    <View style={styles.row} accessibilityLiveRegion="polite">
      {connecting ? (
        <View style={styles.badge}>
          <ActivityIndicator size="small" color={NAVY} />
          <Text style={styles.text}>Connecting privately…</Text>
        </View>
      ) : null}
      {lan ? (
        <View style={[styles.badge, styles.lan]}>
          <Text style={[styles.text, styles.lanText]}>On home Wi-Fi</Text>
        </View>
      ) : null}
      {queued ? (
        <View style={styles.badge}>
          <Text style={styles.text}>{queued === 1 ? "1 change waiting to send" : `${queued} changes waiting to send`}</Text>
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create({
  row: { flexDirection: "row", flexWrap: "wrap", gap: 8, paddingHorizontal: 16, paddingVertical: 8 },
  badge: { flexDirection: "row", alignItems: "center", gap: 6, backgroundColor: SOFT, borderRadius: 999, paddingVertical: 6, paddingHorizontal: 12 },
  text: { color: NAVY, fontSize: 13, fontWeight: "600" },
  lan: { backgroundColor: "#e2f2e8" },
  lanText: { color: GOOD },
});
