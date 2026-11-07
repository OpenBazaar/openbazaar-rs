import { useCallback, useEffect, useRef, useState } from "react";
import { AppState } from "react-native";
import * as LocalAuthentication from "expo-local-authentication";

const RELOCK_AFTER_MS = 5 * 60 * 1000;

/**
 * Locked on open, and again on resume after 5 minutes away, while the phone is paired.
 * `paired` is null until the stored pairing is read. A phone with no screen lock is never locked.
 */
export function useAppLock(paired: boolean | null): { locked: boolean; unlock: () => Promise<void> } {
  const [locked, setLocked] = useState(true);
  const awayAt = useRef<number | null>(null);
  const opened = useRef(false);

  const unlock = useCallback(async () => {
    const level = await LocalAuthentication.getEnrolledLevelAsync();
    if (level === LocalAuthentication.SecurityLevel.NONE) {
      setLocked(false);
      return;
    }
    const result = await LocalAuthentication.authenticateAsync({ promptMessage: "Unlock OpenBazaar" });
    if (result.success) setLocked(false);
  }, []);

  useEffect(() => {
    if (paired === false) setLocked(false);
    if (paired && !opened.current) void unlock();
    if (paired !== null) opened.current = true;
  }, [paired, unlock]);

  useEffect(() => {
    if (!paired) return;
    const subscription = AppState.addEventListener("change", next => {
      if (next === "background") awayAt.current = Date.now();
      if (next === "active" && awayAt.current !== null) {
        if (Date.now() - awayAt.current > RELOCK_AFTER_MS) {
          setLocked(true);
          void unlock();
        }
        awayAt.current = null;
      }
    });
    return () => subscription.remove();
  }, [paired, unlock]);

  return { locked: locked && paired !== false, unlock };
}
