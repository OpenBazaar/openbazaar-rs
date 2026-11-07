import { useCallback, useEffect, useState } from "react";
import { Alert, Platform, ScrollView, StyleSheet, Switch, Text, View } from "react-native";

import { allowNotifications, turnPushOff, turnPushOn } from "../push.ts";
import { fetchDevices, removeDevice, type Device } from "../remoteApi.ts";
import { forget, useSession } from "../session.ts";
import { ago, Button, Card, LINE, NAVY, Notice, ui } from "../ui.tsx";
import { PairAnother } from "./PairAnother.tsx";

const VIA: Record<string, string> = { tor: "Tor", lan: "home Wi-Fi", local: "on the node" };

function DeviceRow({ device, onRemove }: { device: Device; onRemove: () => void }) {
  return (
    <View style={styles.device}>
      <View style={styles.deviceHead}>
        <Text style={styles.deviceName}>
          {device.name}
          {device.thisDevice ? " (this phone)" : ""}
        </Text>
        <Text style={ui.muted}>{device.platform || device.kind}</Text>
      </View>
      <Text style={ui.muted}>
        Last seen {ago(device.lastSeen)}
        {device.lastVia ? ` over ${VIA[device.lastVia]}` : ""} · push {device.push.enabled ? "on" : "off"}
      </Text>
      <Button label={device.thisDevice ? "Remove this phone" : "Remove"} kind="danger" onPress={onRemove} />
    </View>
  );
}

export function SettingsScreen({ onMarket }: { onMarket: () => void }) {
  const session = useSession();
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pushBusy, setPushBusy] = useState(false);
  const [pairing, setPairing] = useState(false);

  const load = useCallback(async () => {
    try {
      setDevices(await fetchDevices());
      setError(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : "The device list did not load.");
    }
  }, []);
  useEffect(() => void load(), [load]);

  const confirmRemove = (device: Device) => {
    const removeIt = async (rotate: boolean) => {
      try {
        if (device.thisDevice) await forget(true);
        else await removeDevice(device.id, rotate);
        await load();
      } catch (err) {
        setError(err instanceof Error ? err.message : "The device was not removed.");
      }
    };
    Alert.alert(`Remove ${device.name}?`, "It loses access at once, with every device it paired. If it was lost or stolen, also move your node to a new address.", [
      { text: "Cancel", style: "cancel" },
      { text: "Remove", style: "destructive", onPress: () => void removeIt(false) },
      ...(device.thisDevice ? [] : [{ text: "Remove and move address", style: "destructive" as const, onPress: () => void removeIt(true) }]),
    ]);
  };

  const togglePush = async (on: boolean) => {
    setPushBusy(true);
    setError(null);
    try {
      await (on ? turnPushOn() : turnPushOff());
      await load();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Push did not change.");
    } finally {
      setPushBusy(false);
    }
  };

  if (pairing) return <PairAnother onDone={() => (setPairing(false), void load())} />;

  return (
    <ScrollView contentContainerStyle={ui.screen}>
      <Text style={ui.title}>Settings</Text>
      {error ? <Notice tone="error" text={error} /> : null}
      <Card title="Notifications">
        <View style={styles.switchRow}>
          <Text style={styles.switchLabel}>Wake this phone when something changes</Text>
          <Switch value={session.pairing?.pushEnabled ?? false} disabled={pushBusy} onValueChange={on => void togglePush(on)} />
        </View>
        <Text style={ui.muted}>
          Off by default. When on, your node sends a content-free wake-up through Apple or Google; the phone then syncs over Tor and shows only “Something changed in your store”.
          {Platform.OS === "android" ? " Without push, the app still checks in the background about every 15 minutes." : " Without push, the app syncs when you open it."}
        </Text>
        {Platform.OS === "android" ? <Button label="Allow new-order notices" kind="plain" onPress={() => void allowNotifications()} /> : null}
      </Card>
      <Card title="Devices">
        {devices === null && !error ? <Text style={ui.muted}>Loading…</Text> : null}
        {devices?.map(device => <DeviceRow key={device.id} device={device} onRemove={() => confirmRemove(device)} />)}
        <Button label="Pair another device" onPress={() => setPairing(true)} />
      </Card>
      <Card title="Marketplace">
        <Button label="Browse and buy" kind="plain" onPress={onMarket} />
      </Card>
      <Card title="This phone">
        <Button
          label="Forget this node"
          kind="danger"
          onPress={() =>
            Alert.alert("Forget your home node?", "This phone asks the node to remove it, then deletes its key. You can pair again with a new code.", [
              { text: "Cancel", style: "cancel" },
              { text: "Forget", style: "destructive", onPress: () => void forget(true) },
            ])
          }
        />
      </Card>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  device: { gap: 6, paddingVertical: 10, borderBottomWidth: 1, borderBottomColor: LINE },
  deviceHead: { flexDirection: "row", justifyContent: "space-between", gap: 8 },
  deviceName: { fontSize: 16, fontWeight: "600", color: NAVY, flexShrink: 1 },
  switchRow: { flexDirection: "row", alignItems: "center", justifyContent: "space-between", gap: 12 },
  switchLabel: { fontSize: 16, color: NAVY, flex: 1 },
});
