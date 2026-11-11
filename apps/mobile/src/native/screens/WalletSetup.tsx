import { useState } from "react";
import { Text, TextInput, View } from "react-native";

import { confirmBackup, passphraseError, setupWallet } from "../../walletLock.ts";
import { Button, Card, Notice, ui } from "../ui.tsx";

type Backup = { phrase: string[]; confirmWords: number[] };

// The node shows the phrase to a paired device once. It is held here, outside
// the screen, so changing tabs or the app lock does not lose it before the
// words are confirmed.
let pending: Backup | null = null;

export const backupPending = () => pending !== null;

/** First-run wallet lock for a node with no screen of its own: passphrase, then the backup words. */
export function WalletSetup({ minChars, onChange }: { minChars: number; onChange: () => void }) {
  const [backup, setBackup] = useState(pending);
  const [passphrase, setPassphrase] = useState("");
  const [repeat, setRepeat] = useState("");
  const [done, setDone] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);

  const keep = (next: Backup | null) => {
    pending = next;
    setBackup(next);
    setDone(next === null);
    onChange();
  };
  const run = async (action: () => Promise<void>, fallback: string) => {
    setBusy(true);
    setMessage(null);
    try {
      await action();
    } catch (err) {
      setMessage(passphraseError(err, fallback));
    } finally {
      setBusy(false);
    }
  };

  if (backup) {
    // The node asks for three of the words back. With none to check, writing them down is the end.
    const confirm = (words: string[]) =>
      run(async () => {
        if (words.length) await confirmBackup(words);
        keep(null);
      }, "The words could not be checked.");
    return <BackupWords backup={backup} busy={busy} message={message} onConfirm={words => void confirm(words)} />;
  }

  // The screen drops this card once the node's new lock state has synced.
  if (done) return <Notice text="The wallet passphrase is set, and the wallet is unlocked." />;

  const short = passphrase.length < minChars;
  const create = () =>
    run(async () => {
      const created = await setupWallet(passphrase);
      setPassphrase("");
      setRepeat("");
      keep({ phrase: created.phrase, confirmWords: created.confirmWords });
    }, "The wallet passphrase could not be set.");
  return (
    <Card title="Protect this node's wallet">
      <Text style={ui.muted}>Choose a wallet passphrase. Your node asks for it to unlock the wallet and before anything is spent or ruled on. Until it is set, the node does neither.</Text>
      <TextInput style={ui.input} value={passphrase} onChangeText={setPassphrase} secureTextEntry autoCapitalize="none" placeholder="Wallet passphrase" accessibilityLabel="Wallet passphrase" />
      <TextInput style={ui.input} value={repeat} onChangeText={setRepeat} secureTextEntry autoCapitalize="none" placeholder="Repeat passphrase" accessibilityLabel="Repeat passphrase" />
      {passphrase && short ? <Text style={ui.muted}>Use at least {minChars} characters.</Text> : null}
      {repeat && passphrase !== repeat ? <Text style={ui.muted}>The passphrases do not match.</Text> : null}
      {message ? <Notice tone="error" text={message} /> : null}
      <Button label="Set passphrase" busy={busy} disabled={short || passphrase !== repeat} onPress={() => void create()} />
    </Card>
  );
}

function BackupWords({ backup, busy, message, onConfirm }: { backup: Backup; busy: boolean; message: string | null; onConfirm: (words: string[]) => void }) {
  const [written, setWritten] = useState(false);
  const [words, setWords] = useState(() => backup.confirmWords.map(() => ""));
  if (!written) {
    return (
      <Card title="Write down the backup phrase">
        <Text style={ui.muted}>These {backup.phrase.length} words restore this node's wallet. Anyone with them can spend from it. A phone is shown them only now, so write them down before you leave the app.</Text>
        <View style={{ gap: 6 }}>
          {backup.phrase.map((word, index) => (
            <Text key={index} style={ui.subtitle} selectable>
              {index + 1}. {word}
            </Text>
          ))}
        </View>
        <Button label="I wrote it down" onPress={() => (backup.confirmWords.length ? setWritten(true) : onConfirm([]))} />
      </Card>
    );
  }
  return (
    <Card title="Confirm the backup">
      {backup.confirmWords.map((position, index) => (
        <TextInput
          key={position}
          style={ui.input}
          value={words[index]}
          onChangeText={text => setWords(all => all.map((word, i) => (i === index ? text : word)))}
          autoCapitalize="none"
          autoCorrect={false}
          placeholder={`Word ${position}`}
          accessibilityLabel={`Word ${position}`}
        />
      ))}
      {message ? <Notice tone="error" text={message} /> : null}
      <Button label="Confirm backup" busy={busy} disabled={words.some(word => !word.trim())} onPress={() => onConfirm(words.map(word => word.trim()))} />
      <Button label="Show the words again" kind="plain" onPress={() => setWritten(false)} />
    </Card>
  );
}
