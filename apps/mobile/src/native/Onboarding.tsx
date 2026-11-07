import { useEffect, useRef, useState } from "react";
import { ScrollView, StyleSheet, Text, TextInput, View } from "react-native";
import { CameraView, useCameraPermissions } from "expo-camera";
import * as Device from "expo-device";

import { PairingError, type PairingCode } from "../homeNode/pairing.ts";
import { parsePairingLink } from "../homeNode/protocol.ts";
import { wordsToSecret } from "../homeNode/words.ts";
import { awaitApproval, beginPairing, type PairingDraft, type PairStep } from "./pairFlow.ts";
import { SasScreen } from "./SasScreen.tsx";
import { Button, Card, Notice, NAVY, ui } from "./ui.tsx";

type Phase =
  | { name: "start" }
  | { name: "scan" }
  | { name: "working"; step: PairStep; detail?: string }
  | { name: "sas"; draft: PairingDraft }
  | { name: "ended"; message: string };

const STEPS: Record<PairStep, string> = {
  keys: "Making this phone's key…",
  tor: "Connecting privately to your node over Tor…",
  request: "Asking your node to pair…",
};

function defaultName(): string {
  return (Device.deviceName ?? Device.modelName ?? "My phone").slice(0, 40);
}

export function Onboarding({ initialLink }: { initialLink: string | null }) {
  const [phase, setPhase] = useState<Phase>({ name: "start" });
  const [words, setWords] = useState("");
  const [name, setName] = useState(defaultName);
  const [error, setError] = useState<string | null>(null);
  const [permission, requestPermission] = useCameraPermissions();
  const cancelled = useRef(false);
  const usedLink = useRef<string | null>(null);

  useEffect(() => () => void (cancelled.current = true), []);

  const pair = async (code: PairingCode) => {
    setError(null);
    const label = name.trim();
    if (!label || label.length > 40) {
      setError("Give this phone a name of 1 to 40 characters.");
      return;
    }
    cancelled.current = false;
    try {
      const draft = await beginPairing(code, label, (step, detail) => setPhase({ name: "working", step, detail }));
      setPhase({ name: "sas", draft });
      const outcome = await awaitApproval(draft, () => cancelled.current);
      if (outcome === "rejected") setPhase({ name: "ended", message: "Your node turned this phone down. Make a new code on the node to try again." });
      if (outcome === "expired") setPhase({ name: "ended", message: "The code ran out before it was approved. Make a new one on your node." });
    } catch (err) {
      setPhase({ name: "start" });
      setError(err instanceof PairingError || err instanceof Error ? err.message : "Pairing failed.");
    }
  };

  const fromLink = (text: string) => {
    const link = parsePairingLink(text);
    if (!link) {
      setError("That QR code is not an OpenBazaar pairing code.");
      setPhase({ name: "start" });
      return;
    }
    void pair({ secret: link.secret, nodePublic: link.nodePublic, onion: link.onion, lan: link.lan });
  };

  useEffect(() => {
    if (initialLink && usedLink.current !== initialLink && phase.name === "start") {
      usedLink.current = initialLink;
      fromLink(initialLink);
    }
  });

  if (phase.name === "sas") {
    return (
      <SasScreen
        pictures={phase.draft.pending.sas}
        via={phase.draft.pending.route.via}
        hardware={phase.draft.hardware}
        onCancel={() => {
          cancelled.current = true;
          setPhase({ name: "start" });
        }}
      />
    );
  }

  if (phase.name === "scan") {
    return (
      <View style={styles.scan}>
        <CameraView
          style={StyleSheet.absoluteFill}
          facing="back"
          barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
          onBarcodeScanned={({ data }) => {
            if (phase.name === "scan") fromLink(data);
          }}
        />
        <View style={styles.scanFooter}>
          <Text style={styles.scanText}>Point the camera at the QR code your node shows.</Text>
          <Button label="Cancel" kind="plain" onPress={() => setPhase({ name: "start" })} />
        </View>
      </View>
    );
  }

  const working = phase.name === "working";
  return (
    <ScrollView contentContainerStyle={ui.screen} keyboardShouldPersistTaps="handled">
      <Text style={ui.title}>Connect to your home node</Text>
      <Text style={ui.subtitle}>
        On the computer that runs your store, run <Text style={styles.code}>ob-node pair</Text> or open Pair a device. It shows a QR code and 8 words.
      </Text>
      {phase.name === "ended" ? <Notice text={phase.message} /> : null}
      {error ? <Notice text={error} tone="error" /> : null}
      <Card title="This phone's name">
        <TextInput style={ui.input} value={name} onChangeText={setName} maxLength={40} editable={!working} accessibilityLabel="This phone's name" />
      </Card>
      <Card title="Scan">
        <Button
          label="Scan the QR code"
          disabled={working}
          onPress={async () => {
            const granted = permission?.granted || (await requestPermission()).granted;
            if (granted) setPhase({ name: "scan" });
            else setError("Allow camera access in Settings to scan, or type the 8 words instead.");
          }}
        />
      </Card>
      <Card title="Or type the 8 words">
        <TextInput
          style={[ui.input, styles.words]}
          value={words}
          onChangeText={setWords}
          placeholder="first four letters of each word are enough"
          autoCapitalize="none"
          autoCorrect={false}
          multiline
          editable={!working}
          accessibilityLabel="Pairing words"
        />
        <Button
          label="Pair with these words"
          disabled={working || !words.trim()}
          onPress={() => {
            const result = wordsToSecret(words);
            if ("error" in result) setError(result.error);
            else void pair({ secret: result.secret, lan: [] });
          }}
        />
      </Card>
      {working ? <Notice text={`${STEPS[phase.step]}${phase.detail ? ` ${phase.detail}` : ""}`} /> : null}
      <Text style={ui.muted}>
        Your phone talks to your node over Tor, or directly on your home Wi-Fi when it can. There is no account and no OpenBazaar server in between.
      </Text>
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  code: { fontFamily: "Menlo", color: NAVY },
  words: { minHeight: 80, textAlignVertical: "top" },
  scan: { flex: 1, backgroundColor: "#000" },
  scanFooter: { position: "absolute", left: 0, right: 0, bottom: 0, padding: 20, gap: 12, backgroundColor: "rgba(0,0,0,0.55)" },
  scanText: { color: "#fff", fontSize: 16, textAlign: "center" },
});
