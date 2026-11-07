import "./polyfills.ts";
import "./push.ts";
import "./background.ts";

import { useEffect, useState, type ComponentType } from "react";
import { ActivityIndicator, AppState, Image, Linking, Pressable, StyleSheet, Text, View } from "react-native";
import { SafeAreaProvider, SafeAreaView } from "react-native-safe-area-context";

import { scheduleBackgroundChecks } from "./background.ts";
import { ConnectionBadge } from "./ConnectionBadge.tsx";
import { Onboarding } from "./Onboarding.tsx";
import { listenForWakes } from "./push.ts";
import { NodeScreen } from "./screens/NodeScreen.tsx";
import { OrdersScreen } from "./screens/OrdersScreen.tsx";
import { SettingsScreen } from "./screens/SettingsScreen.tsx";
import { StoreScreen } from "./screens/StoreScreen.tsx";
import { WalletScreen } from "./screens/WalletScreen.tsx";
import { boot, sync, useSession } from "./session.ts";
import { Button, LINE, MUTED, NAVY, PAGE, ui } from "./ui.tsx";
import { useAppLock } from "./useAppLock.ts";

type Tab = "store" | "orders" | "wallet" | "node" | "settings";
const TABS: { id: Tab; label: string }[] = [
  { id: "store", label: "Store" },
  { id: "orders", label: "Orders" },
  { id: "wallet", label: "Wallet" },
  { id: "node", label: "Node" },
  { id: "settings", label: "Settings" },
];
const SYNC_EVERY_MS = 2 * 60 * 1000;

function usePairingLink(): string | null {
  const [link, setLink] = useState<string | null>(null);
  useEffect(() => {
    void Linking.getInitialURL().then(url => url && setLink(url));
    const subscription = Linking.addEventListener("url", ({ url }) => setLink(url));
    return () => subscription.remove();
  }, []);
  return link && /^openbazaar:\/\/pair\?/i.test(link) ? link : null;
}

/** Sync on open, on every return to the foreground, and every two minutes while open. */
function useSyncWhileOpen(paired: boolean) {
  useEffect(() => {
    if (!paired) return;
    void sync();
    void scheduleBackgroundChecks().catch(() => null);
    const stopWakes = listenForWakes();
    const timer = setInterval(() => AppState.currentState === "active" && void sync(), SYNC_EVERY_MS);
    const subscription = AppState.addEventListener("change", next => next === "active" && void sync());
    return () => {
      stopWakes();
      clearInterval(timer);
      subscription.remove();
    };
  }, [paired]);
}

function Locked({ onUnlock }: { onUnlock: () => void }) {
  return (
    <View style={styles.center}>
      <Image source={require("../../public/brand/ob-market-logo-256.png")} style={styles.logo} accessibilityLabel="OpenBazaar" />
      <Text style={ui.title}>OpenBazaar is locked</Text>
      <Button label="Unlock" onPress={onUnlock} />
    </View>
  );
}

export function HomeNodeApp({ Market }: { Market: ComponentType }) {
  const session = useSession();
  const { locked, unlock } = useAppLock(session.status === "loading" ? null : session.status === "paired");
  const link = usePairingLink();
  const [tab, setTab] = useState<Tab>("store");
  const [market, setMarket] = useState(false);
  useEffect(() => void boot(), []);
  useSyncWhileOpen(session.status === "paired" && !locked);

  let body;
  if (session.status === "loading") body = <ActivityIndicator style={styles.center} color={NAVY} />;
  else if (locked) body = <Locked onUnlock={() => void unlock()} />;
  else if (session.status === "unpaired") body = <Onboarding initialLink={link} />;
  else if (market) {
    body = (
      <View style={styles.fill}>
        <Pressable accessibilityRole="button" onPress={() => setMarket(false)} style={styles.back}>
          <Text style={styles.backText}>‹ Back to your store</Text>
        </Pressable>
        <Market />
      </View>
    );
  } else {
    const refresh = () => void sync();
    body = (
      <View style={styles.fill}>
        <ConnectionBadge />
        <View style={styles.fill}>
          {tab === "store" ? <StoreScreen onRefresh={refresh} /> : null}
          {tab === "orders" ? <OrdersScreen onRefresh={refresh} /> : null}
          {tab === "wallet" ? <WalletScreen /> : null}
          {tab === "node" ? <NodeScreen /> : null}
          {tab === "settings" ? <SettingsScreen onMarket={() => setMarket(true)} /> : null}
        </View>
        <View style={styles.tabs} accessibilityRole="tablist">
          {TABS.map(item => (
            <Pressable key={item.id} accessibilityRole="tab" accessibilityState={{ selected: tab === item.id }} onPress={() => setTab(item.id)} style={styles.tab}>
              <Text style={[styles.tabText, tab === item.id && styles.tabOn]}>{item.label}</Text>
            </Pressable>
          ))}
        </View>
      </View>
    );
  }

  return (
    <SafeAreaProvider>
      <SafeAreaView style={styles.page} edges={["top", "bottom"]}>
        {body}
      </SafeAreaView>
    </SafeAreaProvider>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1, backgroundColor: PAGE },
  fill: { flex: 1 },
  center: { flex: 1, alignItems: "center", justifyContent: "center", gap: 18, padding: 24 },
  logo: { width: 96, height: 96 },
  tabs: { flexDirection: "row", borderTopWidth: 1, borderTopColor: LINE, backgroundColor: "#fff" },
  tab: { flex: 1, alignItems: "center", paddingVertical: 12 },
  tabText: { fontSize: 13, fontWeight: "600", color: MUTED },
  tabOn: { color: NAVY },
  back: { padding: 12 },
  backText: { color: NAVY, fontSize: 16, fontWeight: "600" },
});
