import { ActivityIndicator, ScrollView, StyleSheet, Text, View } from "react-native";

import { Button, CARD, LINE, MUTED, NAVY, Notice, ui } from "./ui.tsx";

export function SasScreen({ pictures, via, hardware, onCancel }: { pictures: { emoji: string; name: string }[]; via: "lan" | "tor"; hardware: boolean; onCancel: () => void }) {
  return (
    <ScrollView contentContainerStyle={ui.screen}>
      <Text style={ui.title}>Check the pictures</Text>
      <Text style={ui.subtitle}>Check that your node shows the same four pictures, then approve it there.</Text>
      <View style={styles.grid} accessibilityLabel={`Pictures: ${pictures.map(p => p.name).join(", ")}`}>
        {pictures.map((picture, i) => (
          <View key={i} style={styles.tile}>
            <Text style={styles.emoji}>{picture.emoji}</Text>
            <Text style={styles.name}>{picture.name}</Text>
          </View>
        ))}
      </View>
      <View style={styles.waiting}>
        <ActivityIndicator color={NAVY} />
        <Text style={ui.muted}>Waiting for approval on your node{via === "lan" ? " (on your home Wi-Fi)" : " (over Tor)"}…</Text>
      </View>
      <Notice text="If the pictures are different, do not approve. Someone else may be using your code: cancel here and make a new code on the node." />
      {!hardware ? <Notice text="This device has no secure hardware for the key (simulator). A real phone keeps it in the Secure Enclave or StrongBox." /> : null}
      <Button label="Cancel" kind="plain" onPress={onCancel} />
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  grid: { flexDirection: "row", flexWrap: "wrap", justifyContent: "space-between", gap: 12 },
  tile: { flexBasis: "46%", flexGrow: 1, minHeight: 168, padding: 16, backgroundColor: CARD, borderRadius: 18, borderWidth: 1, borderColor: LINE, alignItems: "center", justifyContent: "center", gap: 10 },
  emoji: { fontSize: 56 },
  name: { fontSize: 20, lineHeight: 27, textAlign: "center", fontWeight: "600", color: MUTED },
  waiting: { flexDirection: "row", alignItems: "center", gap: 10 },
});
