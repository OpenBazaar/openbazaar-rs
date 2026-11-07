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
import { NativeIcon, type IconName } from "./icons.tsx";
import { NodeScreen } from "./screens/NodeScreen.tsx";
import { OrdersScreen } from "./screens/OrdersScreen.tsx";
import { SettingsScreen } from "./screens/SettingsScreen.tsx";
import { StoreScreen } from "./screens/StoreScreen.tsx";
import { WalletScreen } from "./screens/WalletScreen.tsx";
import { boot, sync, useSession } from "./session.ts";
import { Button, LINE, MUTED, NAVY, PAGE, ui } from "./ui.tsx";
import { useAppLock } from "./useAppLock.ts";

type Tab = "shop" | "store" | "orders" | "wallet" | "node" | "settings";
const TABS: { id: Tab; label: string; icon: IconName }[] = [
  { id: "shop", label: "Shop", icon: "compass" },
  { id: "store", label: "Store", icon: "bag" },
  { id: "orders", label: "Orders", icon: "book" },
  { id: "wallet", label: "Wallet", icon: "wallet" },
  { id: "node", label: "Node", icon: "globe" },
  { id: "settings", label: "Settings", icon: "settings" },
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
  const [tab, setTab] = useState<Tab>("shop");
  useEffect(() => void boot(), []);
  useSyncWhileOpen(session.status === "paired" && !locked);

  let body;
  if (session.status === "loading") body = <ActivityIndicator style={styles.center} color={NAVY} />;
  else if (locked) body = <Locked onUnlock={() => void unlock()} />;
  else if (session.status === "unpaired") body = <Onboarding initialLink={link} />;
  else {
    const refresh = () => void sync();
    body = (
      <View style={styles.fill}>
        <ConnectionBadge />
        <View style={styles.fill}>
          <View style={[styles.fill, tab !== "shop" && styles.hidden]}>
            <Market />
          </View>
          {tab === "store" ? <StoreScreen onRefresh={refresh} /> : null}
          {tab === "orders" ? <OrdersScreen onRefresh={refresh} /> : null}
          {tab === "wallet" ? <WalletScreen /> : null}
          {tab === "node" ? <NodeScreen /> : null}
          {tab === "settings" ? <SettingsScreen /> : null}
        </View>
        <View style={styles.tabs} accessibilityRole="tablist">
          {TABS.map(item => {
            const selected = tab === item.id;
            return (
              <Pressable key={item.id} accessibilityRole="tab" accessibilityLabel={item.label} accessibilityState={{ selected }} onPress={() => setTab(item.id)} style={styles.tab}>
                <NativeIcon name={item.icon} size={22} color={selected ? NAVY : MUTED} />
                <Text style={[styles.tabText, selected && styles.tabOn]}>{item.label}</Text>
              </Pressable>
            );
          })}
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
  hidden: { display: "none" },
  center: { flex: 1, alignItems: "center", justifyContent: "center", gap: 18, padding: 24 },
  logo: { width: 96, height: 96 },
  tabs: { flexDirection: "row", borderTopWidth: 1, borderTopColor: LINE, backgroundColor: "#fff" },
  tab: { flex: 1, minHeight: 49, alignItems: "center", justifyContent: "center", gap: 2, paddingTop: 6, paddingBottom: 4 },
  tabText: { fontSize: 11, fontWeight: "600", color: MUTED },
  tabOn: { color: NAVY },
});
