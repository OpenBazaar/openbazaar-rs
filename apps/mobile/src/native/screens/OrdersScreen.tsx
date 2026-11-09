import { useMemo } from "react";
import { ScrollView, StyleSheet, Text, View } from "react-native";

import { parseBuyerOrder, type BuyerOrder } from "../../catalog.ts";
import { collectionValue } from "../../homeNode/replica.ts";
import { useSession } from "../session.ts";
import { Button, Card, formatBtc, LINE, NAVY, Notice, ui } from "../ui.tsx";

function useOrders(): { orders: BuyerOrder[]; unreadable: number; synced: boolean } {
  const { replica } = useSession();
  return useMemo(() => {
    const value = collectionValue(replica, "orders");
    const raw = Array.isArray(value?.orders) ? value.orders : [];
    const orders: BuyerOrder[] = [];
    let unreadable = 0;
    for (const item of raw) {
      try {
        orders.push(parseBuyerOrder(item));
      } catch {
        unreadable++;
      }
    }
    orders.sort((a, b) => b.created - a.created);
    return { orders, unreadable, synced: value !== null };
  }, [replica]);
}

export function OrdersScreen({ onRefresh }: { onRefresh: () => void }) {
  const session = useSession();
  const { orders, unreadable, synced } = useOrders();
  const error = session.replica.collections.orders?.error;
  return (
    <ScrollView contentContainerStyle={ui.screen}>
      <Text style={ui.title}>Orders</Text>
      {error ? <Notice tone="error" text={`The last sync could not read orders (${error.code}). Showing what this phone has.`} /> : null}
      {!synced ? <Notice text="Orders have not synced to this phone yet." /> : null}
      {synced && orders.length === 0 ? <Text style={ui.muted}>No orders yet.</Text> : null}
      {orders.length ? (
        <Card>
          {orders.map(order => (
            <View key={order.id} style={styles.order}>
              <View style={styles.line}>
                <Text style={styles.title}>{order.lines.map(line => `${line.quantity}× ${line.title}`).join(", ")}</Text>
                <Text style={styles.total}>{formatBtc(order.totalSats)}</Text>
              </View>
              <Text style={ui.muted}>
                {order.status} · {order.settlement === "escrow" ? "escrow" : "direct"} · {new Date(order.created * 1000).toLocaleDateString()}
              </Text>
            </View>
          ))}
        </Card>
      ) : null}
      {unreadable ? <Text style={ui.muted}>{unreadable} orders could not be read on this phone.</Text> : null}
      <Button label={session.syncing ? "Syncing…" : "Sync now"} kind="plain" busy={session.syncing} onPress={onRefresh} />
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  order: { gap: 10, paddingVertical: 16, borderBottomWidth: 1, borderBottomColor: LINE },
  line: { flexDirection: "row", flexWrap: "wrap", justifyContent: "space-between", gap: 12 },
  title: { flex: 1, fontSize: 16, fontWeight: "600", color: NAVY },
  total: { fontSize: 16, lineHeight: 23, color: NAVY },
});
