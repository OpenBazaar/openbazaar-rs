import { useEffect, useState } from "react";
import { ScrollView, StyleSheet, Text, View } from "react-native";

import { Qr } from "../../NativePayments.tsx";
import { parsePairingLink } from "../../homeNode/protocol.ts";
import { secretToWords } from "../../homeNode/words.ts";
import { createPairingCode, decidePending, fetchPending, type NewPairingCode, type PendingRequest } from "../remoteApi.ts";
import { Button, Card, NAVY, Notice, ui } from "../ui.tsx";

/** POST /remote/pairing, then the QR and words for the new device. Approve it from the node or from Settings here. */
export function PairAnother({ onDone }: { onDone: () => void }) {
  const [code, setCode] = useState<NewPairingCode | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<PendingRequest[]>([]);
  useEffect(() => {
    createPairingCode().then(setCode, err => setError(err instanceof Error ? err.message : "No code."));
    let stop = false;
    const poll = async () => {
      while (!stop) {
        setPending(await fetchPending().catch(() => []));
        await new Promise(resolve => setTimeout(resolve, 4000));
      }
    };
    void poll();
    return () => void (stop = true);
  }, []);
  const decide = async (request: PendingRequest, approve: boolean) => {
    try {
      await decidePending(request.requestId, approve);
      setPending(pending.filter(item => item.requestId !== request.requestId));
    } catch (err) {
      setError(err instanceof Error ? err.message : "The node did not take the decision.");
    }
  };
  const link = code?.link ? parsePairingLink(code.link) : null;
  const words = code?.words.length ? code.words : link ? secretToWords(link.secret) : [];
  return (
    <ScrollView contentContainerStyle={ui.screen}>
      <Text style={ui.title}>Pair another device</Text>
      {error ? <Notice tone="error" text={error} /> : null}
      {!code && !error ? <Notice text="Asking your node for a code…" /> : null}
      {code ? (
        <>
          {code.link ? (
            <Card title="Scan with the other device">
              <View style={styles.qr}>
                <Qr value={code.link} />
              </View>
            </Card>
          ) : null}
          {words.length ? (
            <Card title="Or type these 8 words">
              <Text style={styles.words} selectable>
                {words.join("  ")}
              </Text>
            </Card>
          ) : null}
          <Text style={ui.muted}>
            The code works once and for {Math.round(code.expiresInSecs / 60)} minutes. The other device then shows four pictures; check that your node shows the same four before you approve it there.
          </Text>
        </>
      ) : null}
      {pending.map(request => (
        <Card key={request.requestId} title={`${request.name} (${request.platform}) wants to pair`}>
          <Text style={styles.sas}>{request.sas.join("  ")}</Text>
          <Text style={ui.muted}>{request.sasNames.join(", ")}. Approve only if the new device shows these same four pictures.</Text>
          <Button label="Approve" onPress={() => void decide(request, true)} />
          <Button label="Reject" kind="danger" onPress={() => void decide(request, false)} />
        </Card>
      ))}
      <Button label="Done" kind="plain" onPress={onDone} />
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  qr: { alignItems: "center" },
  sas: { fontSize: 44, textAlign: "center" },
  words: { fontSize: 22, lineHeight: 34, fontWeight: "600", color: NAVY, fontFamily: "Menlo" },
});
